use super::{LogLevel, RunnerEvent};
use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::thread;
use strip_ansi_escapes::strip_str;

pub(super) fn spawn_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    is_stderr: bool,
    tx: Sender<RunnerEvent>,
) -> std::io::Result<()> {
    let stream = if is_stderr { "stderr" } else { "stdout" };
    thread::Builder::new()
        .name(if is_stderr {
            "maa-log-stderr".to_string()
        } else {
            "maa-log-stdout".to_string()
        })
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        while matches!(line.last(), Some(b'\n' | b'\r')) {
                            line.pop();
                        }
                        let raw = String::from_utf8_lossy(&line);
                        let (level, text) = classify_log_line(&raw, is_stderr);
                        if tx.send(RunnerEvent::Line { level, text }).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(RunnerEvent::LogReaderFailed {
                            stream,
                            error: error.to_string(),
                        });
                        break;
                    }
                }
            }
        })
        .map(|_| ())
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
