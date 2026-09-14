//! 子进程控制：启动 `maa`、捕获分级日志、停止进程组。

mod command;
mod core_progress;
mod log;
mod process;

pub use command::{TaskCommand, TaskKind};
#[allow(unused_imports)]
pub use log::classify_log_line;
pub use process::RunningTask;

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
    LogReaderFailed { stream: &'static str, error: String },
    DailyTaskStarted { task_id: usize, taskchain: String },
    CopilotStageSucceeded,
    ProgressFailed(String),
    Exited { code: Option<i32>, stopped: bool },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::core_progress::{CoreLogCursor, CoreProgressEvent, FileIdentity};
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};
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
        assert_eq!(batch.kind, TaskKind::Copilot);
        assert!(batch.tracks_copilot_progress());

        let roguelike = TaskCommand::roguelike(
            "maatui-roguelike-test",
            std::path::PathBuf::from("/tmp/roguelike.json"),
        );
        assert_eq!(
            roguelike.args,
            ["run", "maatui-roguelike-test", "--batch", "-v"]
        );
        assert_eq!(roguelike.kind, TaskKind::Roguelike);
        assert_eq!(roguelike.cleanup, Some("/tmp/roguelike.json".into()));
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
            track_daily: false,
            track_copilot: true,
        };
        let success = r#"[INF][Px123][Tx1] Assistant::append_callback | SubTaskStart {"details":{"task":"StageDrops-Stars-3"},"taskchain":"Copilot"}"#;
        assert_eq!(
            cursor.parse_progress_line(success),
            Some(CoreProgressEvent::CopilotStageSucceeded)
        );
        assert!(
            cursor
                .parse_progress_line(&success.replace("Px123", "Px124"))
                .is_none()
        );
        assert!(
            cursor
                .parse_progress_line(&success.replace("SubTaskStart", "SubTaskCompleted"))
                .is_none()
        );
        assert!(
            cursor
                .parse_progress_line(&success.replace("Copilot", "Mall"))
                .is_none()
        );
    }

    #[test]
    fn core_log_cursor_parses_daily_task_start_for_matching_pid() {
        let cursor = CoreLogCursor {
            path: PathBuf::new(),
            backup_path: PathBuf::new(),
            offset: 0,
            identity: None,
            pid: 321,
            partial: String::new(),
            track_daily: true,
            track_copilot: false,
        };
        let line = r#"[INF][Px321][Tx1] Assistant::append_callback | TaskChainStart {"taskchain":"Fight","taskid":3}"#;
        assert_eq!(
            cursor.parse_progress_line(line),
            Some(CoreProgressEvent::DailyTaskStarted {
                task_id: 3,
                taskchain: "Fight".to_string(),
            })
        );
        assert!(
            cursor
                .parse_progress_line(&line.replace("Px321", "Px999"))
                .is_none()
        );
    }

    #[test]
    fn core_log_cursor_starts_after_existing_log_content() {
        let dir = temp_dir();
        let path = dir.join("asst.log");
        fs::write(&path, "history\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();

        let cursor = CoreLogCursor::prepare_in_dir(&dir, true, false).unwrap();

        assert_eq!(cursor.offset, metadata.len());
        assert_eq!(
            cursor.identity,
            Some(FileIdentity::from_metadata(&metadata))
        );
        assert_eq!(cursor.path, path);
        assert_eq!(cursor.backup_path, dir.join("asst.bak.log"));
        fs::remove_dir_all(dir).unwrap();
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
            track_daily: false,
            track_copilot: true,
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
        assert_eq!(cursor.poll().unwrap().len(), 1);

        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(success("StageDrops-Stars-Adverse").as_bytes())
            .unwrap();
        fs::rename(&path, &backup_path).unwrap();
        fs::write(&path, success("StageDrops-Stars-3")).unwrap();

        assert_eq!(cursor.poll().unwrap().len(), 2);
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
                    RunnerEvent::LogReaderFailed { .. } => {}
                    RunnerEvent::DailyTaskStarted { .. } => {}
                    RunnerEvent::CopilotStageSucceeded => {}
                    RunnerEvent::ProgressFailed(_) => {}
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
    fn preserves_lines_with_invalid_utf8() {
        let mut task =
            RunningTask::spawn_command("sh", &["-c", "printf '\\377\\n'"]).expect("spawn sh");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_replacement = false;
        while Instant::now() < deadline {
            for event in task.poll_events() {
                if let RunnerEvent::Line { text, .. } = event {
                    saw_replacement = text.contains('�');
                }
            }
            if saw_replacement {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(saw_replacement);
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
        assert_eq!(
            classify_log_line("[WARNING] caution", false).0,
            LogLevel::Warn
        );
        assert_eq!(classify_log_line("[INFO] normal", true).0, LogLevel::Info);
        assert_eq!(classify_log_line("[DEBUG] detail", true).0, LogLevel::Debug);
        assert_eq!(classify_log_line("[TRACE] trace", true).0, LogLevel::Trace);
        assert_eq!(classify_log_line("[ERR] boom", false).0, LogLevel::Error);
        assert_eq!(
            classify_log_line("[CRITICAL] boom", false).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("thread 'main' panicked at src/main.rs", true).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("错误：连接失败", false).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("[12:00:00] 失败：连接中断", false).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("[12:00:00] PANIC: crashed", true).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("[12:00:00] EXCEPTION: crashed", true).0,
            LogLevel::Error
        );
        assert_eq!(
            classify_log_line("ordinary stderr", true).0,
            LogLevel::Plain
        );
    }
}
