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

/// 仅识别 maa-cli 的完整时间戳及等级前缀，避免把正文中的等级词当作级别。
pub(crate) fn maa_cli_log_parts(text: &str) -> Option<(&str, &str, LogLevel)> {
    let bytes = text.as_bytes();
    if bytes.len() < 27
        || bytes.first() != Some(&b'[')
        || ![1, 2, 3, 4, 6, 7, 9, 10, 12, 13, 15, 16, 18, 19]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
        || ![
            (5, b'-'),
            (8, b'-'),
            (11, b' '),
            (14, b':'),
            (17, b':'),
            (20, b' '),
        ]
        .iter()
        .all(|&(i, value)| bytes[i] == value)
    {
        return None;
    }
    let end = text[21..].find("] ")? + 21;
    // maa-cli 将等级右侧填充到固定宽度（如 "INFO ]"），判定等级时忽略这些填充空格，
    // 但返回的前缀切片保持原文，避免改动用户可见的时间戳格式。
    let level = match text[21..end].trim_end_matches(' ') {
        "ERROR" => LogLevel::Error,
        "WARN" => LogLevel::Warn,
        "INFO" => LogLevel::Info,
        "DEBUG" => LogLevel::Debug,
        "TRACE" => LogLevel::Trace,
        _ => return None,
    };
    Some((&text[..end + 2], &text[end + 2..], level))
}

pub fn classify_log_line(raw: &str, _is_stderr: bool) -> (LogLevel, String) {
    let text = strip_str(raw);
    let upper = text.to_ascii_uppercase();
    let trimmed = text.trim_start();
    let trimmed_upper = upper.trim_start();
    let level = if let Some((_, _, level)) = maa_cli_log_parts(&text) {
        level
    } else if ["ERROR", "ERR", "FATAL", "CRITICAL"]
        .iter()
        .any(|level| contains_level(&upper, level))
        || message_starts_with(trimmed_upper, "PANIC:")
        || upper.contains("PANICKED AT")
        || message_starts_with(trimmed_upper, "EXCEPTION:")
        || ["错误:", "错误：", "失败:", "失败："]
            .iter()
            .any(|prefix| message_starts_with(trimmed, prefix))
    {
        LogLevel::Error
    } else if contains_level(&upper, "WARN") || contains_level(&upper, "WARNING") {
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

fn message_starts_with(text: &str, prefix: &str) -> bool {
    text.starts_with(prefix) || text.contains(&format!("] {prefix}"))
}
