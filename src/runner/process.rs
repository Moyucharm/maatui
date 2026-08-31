use super::core_progress::{CoreLogCursor, CoreProgressEvent};
use super::log::spawn_reader;
use super::{LogLevel, RunnerEvent, TaskCommand};
use libc::{SIGKILL, SIGTERM, kill, pid_t};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

pub struct RunningTask {
    child: Child,
    pgid: pid_t,
    event_rx: Receiver<RunnerEvent>,
    stop_requested: bool,
    term_at: Option<Instant>,
    finished: bool,
    exit_emitted: bool,
    core_log: Option<CoreLogCursor>,
}

impl RunningTask {
    pub fn spawn(command: &TaskCommand) -> Result<Self, String> {
        let track_daily = command.tracks_daily_progress();
        let track_copilot = command.tracks_copilot_progress();
        let core_log = (track_daily || track_copilot)
            .then(|| CoreLogCursor::prepare(track_daily, track_copilot))
            .transpose()?;
        Self::spawn_command_with_env(
            &command.program,
            &command.args.iter().map(String::as_str).collect::<Vec<_>>(),
            &command
                .envs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>(),
            core_log,
        )
    }

    #[cfg(test)]
    pub fn spawn_command(program: &str, args: &[&str]) -> Result<Self, String> {
        Self::spawn_command_with_env(program, args, &[], None)
    }

    fn spawn_command_with_env(
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        mut core_log: Option<CoreLogCursor>,
    ) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<RunnerEvent>();

        let mut cmd = Command::new(program);
        cmd.args(args)
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // 独立进程组，停止时组杀避免 MaaCore 等子进程残留。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = if error.kind() == std::io::ErrorKind::NotFound {
                    format!("找不到命令 `{program}`，请确认已安装并在 PATH 中")
                } else {
                    format!("启动 `{program}` 失败: {error}")
                };
                return Err(message);
            }
        };

        let pgid = child.id() as pid_t;
        if let Some(cursor) = core_log.as_mut() {
            cursor.pid = child.id();
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "缺少 stdout pipe".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "缺少 stderr pipe".to_string())?;

        if let Err(error) = spawn_reader(stdout, false, tx.clone()) {
            let _ = unsafe { kill(-pgid, SIGKILL) };
            let _ = child.wait();
            return Err(format!("启动 stdout 日志线程失败: {error}"));
        }
        if let Err(error) = spawn_reader(stderr, true, tx) {
            let _ = unsafe { kill(-pgid, SIGKILL) };
            let _ = child.wait();
            return Err(format!("启动 stderr 日志线程失败: {error}"));
        }

        Ok(Self {
            child,
            pgid,
            event_rx: rx,
            stop_requested: false,
            term_at: None,
            finished: false,
            exit_emitted: false,
            core_log,
        })
    }

    pub fn poll_events(&mut self) -> Vec<RunnerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }
        self.poll_core_log(&mut events);

        if self.stop_requested
            && !self.finished
            && let Some(term_at) = self.term_at
            && term_at.elapsed() >= Duration::from_secs(3)
        {
            let _ = unsafe { kill(-self.pgid, SIGKILL) };
            self.term_at = None;
        }

        if !self.finished {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.finished = true;
                    self.poll_core_log(&mut events);
                    if !self.exit_emitted {
                        self.exit_emitted = true;
                        events.push(RunnerEvent::Exited {
                            code: status.code(),
                            stopped: self.stop_requested,
                        });
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.finished = true;
                    events.push(RunnerEvent::Line {
                        level: LogLevel::Error,
                        text: format!("等待子进程出错: {error}"),
                    });
                    if !self.exit_emitted {
                        self.exit_emitted = true;
                        events.push(RunnerEvent::Exited {
                            code: None,
                            stopped: self.stop_requested,
                        });
                    }
                }
            }
        }

        events
    }

    fn poll_core_log(&mut self, events: &mut Vec<RunnerEvent>) {
        let Some(cursor) = self.core_log.as_mut() else {
            return;
        };
        match cursor.poll() {
            Ok(progress) => events.extend(progress.into_iter().map(|event| match event {
                CoreProgressEvent::DailyTaskStarted { task_id, taskchain } => {
                    RunnerEvent::DailyTaskStarted { task_id, taskchain }
                }
                CoreProgressEvent::CopilotStageSucceeded => RunnerEvent::CopilotStageSucceeded,
            })),
            Err(error) => {
                events.push(RunnerEvent::ProgressFailed(error.to_string()));
                self.core_log = None;
            }
        }
    }

    pub fn request_stop(&mut self) {
        if self.finished || self.stop_requested {
            return;
        }
        self.stop_requested = true;
        self.term_at = Some(Instant::now());
        let _ = unsafe { kill(-self.pgid, SIGTERM) };
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested
    }

    pub fn force_cleanup(mut self) {
        if !self.finished {
            self.request_stop();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() >= deadline {
                            let _ = unsafe { kill(-self.pgid, SIGKILL) };
                            let _ = self.child.wait();
                            break;
                        }
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => {
                        let _ = unsafe { kill(-self.pgid, SIGKILL) };
                        let _ = self.child.wait();
                        break;
                    }
                }
            }
        }
    }
}
