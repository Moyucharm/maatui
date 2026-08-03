//! 子进程控制：启动 `maa`、捕获分级日志、停止进程组。

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
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
    CopilotStageSucceeded,
    CopilotProgressFailed(String),
    Exited { code: Option<i32>, stopped: bool },
}

#[derive(Debug, Clone)]
pub struct TaskCommand {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
    pub track_copilot_progress: bool,
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

    pub fn copilot_batch(task_path: &str) -> Self {
        let mut command = Self::maa("作业集自动战斗", ["run", task_path, "--batch", "-v"]);
        command.track_copilot_progress = true;
        command
    }

    pub fn resource_update() -> Self {
        Self::maa("资源热更新", ["hot-update", "--batch", "-v"])
    }

    pub fn core_update() -> Self {
        Self::maa("Core 与基础资源更新", ["update", "--batch", "-v"])
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
            track_copilot_progress: false,
        }
    }

    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

struct CoreLogCursor {
    path: PathBuf,
    backup_path: PathBuf,
    offset: u64,
    identity: Option<FileIdentity>,
    pid: u32,
    partial: String,
}

impl CoreLogCursor {
    fn prepare(program: &str) -> Result<Self, String> {
        let output = Command::new(program)
            .args(["dir", "log"])
            .output()
            .map_err(|error| format!("无法定位 MaaCore 日志目录: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "无法定位 MaaCore 日志目录: maa dir log exit {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "?".to_string(), |code| code.to_string())
            ));
        }
        let dir = String::from_utf8(output.stdout)
            .map_err(|_| "maa dir log 返回了非 UTF-8 路径".to_string())?;
        let dir = PathBuf::from(dir.trim());
        if dir.as_os_str().is_empty() {
            return Err("maa dir log 返回了空路径".to_string());
        }
        let path = dir.join("asst.log");
        let backup_path = dir.join("asst.bak.log");
        let (offset, identity) = match fs::metadata(&path) {
            Ok(metadata) => (metadata.len(), Some(FileIdentity::from_metadata(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, None),
            Err(error) => return Err(format!("读取 MaaCore 日志失败: {error}")),
        };
        Ok(Self {
            path,
            backup_path,
            offset,
            identity,
            pid: 0,
            partial: String::new(),
        })
    }

    fn poll(&mut self) -> std::io::Result<usize> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let current_identity = FileIdentity::from_metadata(&metadata);
        let mut success_count = 0;
        if self
            .identity
            .is_some_and(|identity| identity != current_identity)
            || metadata.len() < self.offset
        {
            success_count += self.read_rotated_backup()?;
            self.offset = 0;
            self.partial.clear();
        }
        self.identity = Some(current_identity);
        Ok(success_count + self.read_current()?)
    }

    fn read_rotated_backup(&mut self) -> std::io::Result<usize> {
        let Some(identity) = self.identity else {
            return Ok(0);
        };
        let Ok(metadata) = fs::metadata(&self.backup_path) else {
            return Ok(0);
        };
        if FileIdentity::from_metadata(&metadata) != identity || metadata.len() <= self.offset {
            return Ok(0);
        }
        let mut lines = Vec::new();
        self.read_from(self.backup_path.clone(), metadata.len(), &mut lines)?;
        Ok(lines
            .into_iter()
            .filter(|line| self.is_success_line(line))
            .count())
    }

    fn read_current(&mut self) -> std::io::Result<usize> {
        let len = fs::metadata(&self.path)?.len();
        let mut lines = Vec::new();
        self.read_from(self.path.clone(), len, &mut lines)?;
        Ok(lines
            .into_iter()
            .filter(|line| self.is_success_line(line))
            .count())
    }

    fn read_from(
        &mut self,
        path: PathBuf,
        len: u64,
        lines: &mut Vec<String>,
    ) -> std::io::Result<()> {
        if len <= self.offset {
            return Ok(());
        }
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::with_capacity((len - self.offset) as usize);
        file.read_to_end(&mut bytes)?;
        self.offset = len;
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(index) = self.partial.find('\n') {
            let line = self.partial[..index].trim_end_matches('\r').to_string();
            self.partial.drain(..=index);
            lines.push(line);
        }
        Ok(())
    }

    fn is_success_line(&self, line: &str) -> bool {
        line.contains(&format!("[Px{}]", self.pid))
            && line.contains("Assistant::append_callback | SubTaskStart ")
            && line.contains("\"taskchain\":\"Copilot\"")
            && (line.contains("\"task\":\"StageDrops-Stars-3\"")
                || line.contains("\"task\":\"StageDrops-Stars-Adverse\""))
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
    core_log: Option<CoreLogCursor>,
}

impl RunningTask {
    pub fn spawn(command: &TaskCommand) -> Result<Self, String> {
        let core_log = command
            .track_copilot_progress
            .then(|| CoreLogCursor::prepare(&command.program))
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
            Ok(count) => events.extend((0..count).map(|_| RunnerEvent::CopilotStageSucceeded)),
            Err(error) => {
                events.push(RunnerEvent::CopilotProgressFailed(error.to_string()));
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
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maatui-runner-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn manual_update_commands_use_non_interactive_mode() {
        let resource = TaskCommand::resource_update();
        assert_eq!(resource.args, ["hot-update", "--batch", "-v"]);

        let core = TaskCommand::core_update();
        assert_eq!(core.args, ["update", "--batch", "-v"]);

        let batch = TaskCommand::copilot_batch("/tmp/batch.json");
        assert_eq!(batch.args, ["run", "/tmp/batch.json", "--batch", "-v"]);
        assert!(batch.track_copilot_progress);
    }

    #[test]
    fn core_log_cursor_tracks_only_exact_success_callbacks() {
        let cursor = CoreLogCursor {
            path: PathBuf::new(),
            backup_path: PathBuf::new(),
            offset: 0,
            identity: None,
            pid: 123,
            partial: String::new(),
        };
        let success = r#"[INF][Px123][Tx1] Assistant::append_callback | SubTaskStart {"details":{"task":"StageDrops-Stars-3"},"taskchain":"Copilot"}"#;
        assert!(cursor.is_success_line(success));
        assert!(!cursor.is_success_line(&success.replace("Px123", "Px124")));
        assert!(!cursor.is_success_line(&success.replace("SubTaskStart", "SubTaskCompleted")));
        assert!(!cursor.is_success_line(&success.replace("Copilot", "Mall")));
    }

    #[test]
    fn core_log_cursor_survives_log_rotation() {
        let dir = temp_dir();
        let path = dir.join("asst.log");
        let backup_path = dir.join("asst.bak.log");
        fs::write(&path, "initial\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let mut cursor = CoreLogCursor {
            path: path.clone(),
            backup_path: backup_path.clone(),
            offset: metadata.len(),
            identity: Some(FileIdentity::from_metadata(&metadata)),
            pid: 123,
            partial: String::new(),
        };
        let success = |stage: &str| {
            format!(
                "[INF][Px123][Tx1] Assistant::append_callback | SubTaskStart {{\"details\":{{\"task\":\"{stage}\"}},\"taskchain\":\"Copilot\"}}\n"
            )
        };

        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(success("StageDrops-Stars-3").as_bytes())
            .unwrap();
        assert_eq!(cursor.poll().unwrap(), 1);

        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(success("StageDrops-Stars-Adverse").as_bytes())
            .unwrap();
        fs::rename(&path, &backup_path).unwrap();
        fs::write(&path, success("StageDrops-Stars-3")).unwrap();

        assert_eq!(cursor.poll().unwrap(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

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
                    RunnerEvent::CopilotStageSucceeded => {}
                    RunnerEvent::CopilotProgressFailed(_) => {}
                    RunnerEvent::Exited { code, stopped } => exited = Some((code, stopped)),
                }
            }
            if exited.is_some()
                && lines.iter().any(|(_, line)| line.contains("hello-maatui"))
                && lines.iter().any(|(_, line)| line.contains("err-line"))
            {
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
