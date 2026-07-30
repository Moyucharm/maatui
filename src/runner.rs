//! 子进程控制：启动 `maa`、捕获日志、停止进程组。

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use libc::{kill, pid_t, SIGKILL, SIGTERM};
use strip_ansi_escapes::strip_str;

/// 日志来源流。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    System,
}

/// Runner 推送给 UI 的事件。
#[derive(Debug, Clone)]
pub enum RunnerEvent {
    Line { stream: LogStream, text: String },
    Exited { code: Option<i32>, stopped: bool },
}

/// 正在运行的任务句柄。
pub struct RunningTask {
    child: Child,
    pgid: pid_t,
    event_rx: Receiver<RunnerEvent>,
    /// 用户是否请求过停止。
    stop_requested: bool,
    /// 已发送 SIGTERM 的时间；用于超时 SIGKILL。
    term_at: Option<Instant>,
    /// 是否已观察到进程退出。
    finished: bool,
    /// 是否已向 UI 发送过 Exited 事件。
    exit_emitted: bool,
}

impl RunningTask {
    /// 启动 `maa run daily -v`。
    pub fn spawn_daily() -> Result<Self, String> {
        Self::spawn_command("maa", &["run", "daily", "-v"])
    }

    /// 通用启动（测试与正式路径共用）。
    pub fn spawn_command(program: &str, args: &[&str]) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<RunnerEvent>();

        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // 独立进程组，停止时组杀避免孤儿。
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::NotFound {
                    format!("找不到命令 `{program}`，请确认已安装并在 PATH 中")
                } else {
                    format!("启动 `{program}` 失败: {e}")
                };
                return Err(msg);
            }
        };

        // 子进程 pre_exec 中 setpgid(0,0)，故 pgid == pid。
        let pgid = child.id() as pid_t;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "缺少 stdout pipe".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "缺少 stderr pipe".to_string())?;

        spawn_reader(stdout, LogStream::Stdout, tx.clone());
        spawn_reader(stderr, LogStream::Stderr, tx);

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

    /// 非阻塞拉取事件，并推进停止超时 / 退出检测。
    pub fn poll_events(&mut self) -> Vec<RunnerEvent> {
        let mut events = Vec::new();

        while let Ok(ev) = self.event_rx.try_recv() {
            events.push(ev);
        }

        // 停止超时：SIGTERM 后 3s 仍未退出则 SIGKILL。
        if self.stop_requested
            && !self.finished
            && let Some(term_at) = self.term_at
            && term_at.elapsed() >= Duration::from_secs(3)
        {
            let _ = unsafe { kill(-self.pgid, SIGKILL) };
            // 避免重复 kill。
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
                Err(e) => {
                    self.finished = true;
                    events.push(RunnerEvent::Line {
                        stream: LogStream::System,
                        text: format!("等待子进程出错: {e}"),
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

    /// 请求停止：向进程组发送 SIGTERM。
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

    /// 阻塞回收：确保进程结束（App 退出时调用）。
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
    stream: LogStream,
    tx: Sender<RunnerEvent>,
) {
    let _ = thread::Builder::new()
        .name(format!("maa-log-{stream:?}"))
        .spawn(move || {
            let buf = BufReader::new(reader);
            for line in buf.lines() {
                match line {
                    Ok(raw) => {
                        let text = strip_str(&raw);
                        if tx.send(RunnerEvent::Line { stream, text }).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_returns_error() {
        let result = RunningTask::spawn_command("maatui-definitely-missing-bin", &[]);
        let err = match result {
            Ok(_) => panic!("should fail to spawn missing program"),
            Err(e) => e,
        };
        assert!(err.contains("找不到命令"), "err={err}");
    }

    #[test]
    fn captures_stdout_and_exits() {
        let mut task = RunningTask::spawn_command("sh", &["-c", "echo hello-maatui; echo err-line 1>&2"])
            .expect("spawn sh");

        let mut lines = Vec::new();
        let mut exited = None;
        let deadline = Instant::now() + Duration::from_secs(2);

        while Instant::now() < deadline {
            for ev in task.poll_events() {
                match ev {
                    RunnerEvent::Line { text, .. } => lines.push(text),
                    RunnerEvent::Exited { code, stopped } => {
                        exited = Some((code, stopped));
                    }
                }
            }
            if exited.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(lines.iter().any(|l| l.contains("hello-maatui")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("err-line")), "{lines:?}");
        let (code, stopped) = exited.expect("should exit");
        assert_eq!(code, Some(0));
        assert!(!stopped);
        assert!(task.is_finished());
    }

    #[test]
    fn stop_terminates_long_running_process() {
        let mut task = RunningTask::spawn_command("sh", &["-c", "echo start; sleep 30; echo end"])
            .expect("spawn sleep");

        // 等第一行日志，确认已跑起来。
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_start = false;
        while Instant::now() < deadline {
            for ev in task.poll_events() {
                if let RunnerEvent::Line { text, .. } = ev
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
        assert!(saw_start, "should see start line");

        task.request_stop();
        assert!(task.stop_requested());

        let mut exited = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            for ev in task.poll_events() {
                if let RunnerEvent::Exited { code, stopped } = ev {
                    exited = Some((code, stopped));
                }
            }
            if exited.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        let (code, stopped) = exited.expect("should be stopped");
        assert!(stopped);
        // SIGTERM 常见 exit 为 None（信号终止）或非 0。
        let _ = code;
        assert!(task.is_finished());
    }
}
