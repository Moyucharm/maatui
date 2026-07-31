//! 子进程控制：启动 `maa`、捕获分级日志、停止进程组。

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use libc::{SIGKILL, SIGTERM, kill, pid_t};
use strip_ansi_escapes::strip_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Plain,
    Info,
    Success,
    Warn,
    Error,
    Debug,
    Trace,
    System,
}

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    Line { level: LogLevel, text: String },
    Exited { code: Option<i32>, stopped: bool },
}

#[derive(Debug, Clone)]
pub struct TaskCommand {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

impl TaskCommand {
    pub fn daily() -> Self {
        Self::maa("每日任务", ["run", "daily", "-v"])
    }

    pub fn copilot(args: Vec<String>) -> Self {
        let mut command = Self::maa("自动战斗", std::iter::empty::<&str>());
        command.args = args;
        command
    }

    pub fn maa<I, S>(label: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            label: label.into(),
            program: "maa".to_string(),
            args: args.into_iter().map(Into::into).collect(),
            envs: vec![("MAA_LOG_PREFIX".to_string(), "Always".to_string())],
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub struct RunningTask {
    child: Child,
    pgid: pid_t,
    event_rx: Receiver<RunnerEvent>,
    stop_requested: bool,
    term_at: Option<Instant>,
    finished: bool,
    exit_emitted: bool,
}

impl RunningTask {
    pub fn spawn(command: &TaskCommand) -> Result<Self, String> {
        Self::spawn_command_with_env(
            &command.program,
            &command.args.iter().map(String::as_str).collect::<Vec<_>>(),
            &command
                .envs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>(),
        )
    }

    #[cfg(test)]
    pub fn spawn_command(program: &str, args: &[&str]) -> Result<Self, String> {
        Self::spawn_command_with_env(program, args, &[])
    }

    fn spawn_command_with_env(
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
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
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "缺少 stdout pipe".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "缺少 stderr pipe".to_string())?;

        spawn_reader(stdout, false, tx.clone());
        spawn_reader(stderr, true, tx);

        Ok(Self {
            child,
            pgid,
            event_rx: rx,
            stop_requested: false,
            term_at: None,
            finished: false,
            exit_emitted: false,
        })
    }

    pub fn poll_events(&mut self) -> Vec<RunnerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }

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

fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    is_stderr: bool,
    tx: Sender<RunnerEvent>,
) {
    let _ = thread::Builder::new()
        .name(if is_stderr {
            "maa-log-stderr".to_string()
        } else {
            "maa-log-stdout".to_string()
        })
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                match line {
                    Ok(raw) => {
                        let (level, text) = classify_log_line(&raw, is_stderr);
                        if tx.send(RunnerEvent::Line { level, text }).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
}

pub fn classify_log_line(raw: &str, _is_stderr: bool) -> (LogLevel, String) {
    let text = strip_str(raw);
    let upper = text.to_ascii_uppercase();
    let level = if contains_level(&upper, "ERROR") {
        LogLevel::Error
    } else if contains_level(&upper, "WARN") {
        LogLevel::Warn
    } else if contains_level(&upper, "DEBUG") {
        LogLevel::Debug
    } else if contains_level(&upper, "TRACE") {
        LogLevel::Trace
    } else if contains_level(&upper, "INFO") {
        LogLevel::Info
    } else if upper.contains("SUCCESS") || text.contains("完成") || text.contains("成功") {
        LogLevel::Success
    } else {
        // maa-cli 的常规日志也写入 stderr，不能仅凭输出流判定为警告。
        LogLevel::Plain
    };
    (level, text)
}

fn contains_level(text: &str, level: &str) -> bool {
    text.contains(&format!("[{level}]"))
        || text.contains(&format!(" {level} "))
        || text.starts_with(&format!("{level}:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_returns_error() {
        let result = RunningTask::spawn_command("maatui-definitely-missing-bin", &[]);
        let error = match result {
            Ok(_) => panic!("should fail to spawn missing program"),
            Err(error) => error,
        };
        assert!(error.contains("找不到命令"), "error={error}");
    }

    #[test]
    fn captures_stdout_and_exits() {
        let mut task = RunningTask::spawn_command(
            "sh",
            &["-c", "echo hello-maatui; echo '[ERROR] err-line' 1>&2"],
        )
        .expect("spawn sh");

        let mut lines = Vec::new();
        let mut exited = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for event in task.poll_events() {
                match event {
                    RunnerEvent::Line { level, text } => lines.push((level, text)),
                    RunnerEvent::Exited { code, stopped } => exited = Some((code, stopped)),
                }
            }
            if exited.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(lines.iter().any(|(_, line)| line.contains("hello-maatui")));
        assert!(
            lines
                .iter()
                .any(|(level, line)| { *level == LogLevel::Error && line.contains("err-line") })
        );
        assert_eq!(exited, Some((Some(0), false)));
        assert!(task.is_finished());
    }

    #[test]
    fn stop_terminates_long_running_process() {
        let mut task = RunningTask::spawn_command("sh", &["-c", "echo start; sleep 30; echo end"])
            .expect("spawn sleep");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_start = false;
        while Instant::now() < deadline {
            for event in task.poll_events() {
                if let RunnerEvent::Line { text, .. } = event
                    && text.contains("start")
                {
                    saw_start = true;
                }
            }
            if saw_start {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_start);

        task.request_stop();
        let mut exited = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for event in task.poll_events() {
                if let RunnerEvent::Exited { code, stopped } = event {
                    exited = Some((code, stopped));
                }
            }
            if exited.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(exited.expect("should stop").1);
    }

    #[test]
    fn classifies_levels_without_treating_all_stderr_as_warning() {
        assert_eq!(classify_log_line("[ERROR] boom", true).0, LogLevel::Error);
        assert_eq!(classify_log_line("[WARN] caution", true).0, LogLevel::Warn);
        assert_eq!(classify_log_line("[INFO] normal", true).0, LogLevel::Info);
        assert_eq!(classify_log_line("[DEBUG] detail", true).0, LogLevel::Debug);
        assert_eq!(classify_log_line("[TRACE] trace", true).0, LogLevel::Trace);
        assert_eq!(
            classify_log_line("ordinary stderr", true).0,
            LogLevel::Plain
        );
    }
}
