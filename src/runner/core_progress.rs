use crate::storage::maa_log_dir;
use serde_json::Value as JsonValue;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    pub(super) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

pub(super) struct CoreLogCursor {
    pub(super) path: PathBuf,
    pub(super) backup_path: PathBuf,
    pub(super) offset: u64,
    pub(super) identity: Option<FileIdentity>,
    pub(super) pid: u32,
    pub(super) partial: String,
    pub(super) track_daily: bool,
    pub(super) track_copilot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CoreProgressEvent {
    DailyTaskStarted {
        task_id: usize,
        taskchain: String,
    },
    SubTaskFailure {
        task_id: usize,
        taskchain: String,
        reason: String,
    },
    TaskChainFailed {
        task_id: usize,
        taskchain: String,
        error: Option<String>,
    },
    TaskChainSucceeded {
        task_id: usize,
        taskchain: String,
    },
    CopilotTaskStarted {
        task_id: usize,
    },
    DailyTasksCompleted,
    CopilotStageSucceeded,
}
fn missing_operators(payload: &JsonValue) -> String {
    let mut missing = Vec::new();
    let mut unavailable = Vec::new();
    let Some(groups) = payload
        .pointer("/details/opers")
        .and_then(JsonValue::as_object)
    else {
        return "自动编队失败：缺少干员".to_string();
    };
    for (group, operators) in groups {
        let Some(operators) = operators.as_array() else {
            continue;
        };
        for operator in operators {
            let name = operator
                .get("name")
                .and_then(JsonValue::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(group);
            match operator.get("reason").and_then(JsonValue::as_str) {
                Some("Missing" | "Unchecked") => missing.push(name.to_string()),
                Some("Unavailable") => unavailable.push(name.to_string()),
                Some(reason) => missing.push(format!("{name}（{reason}）")),
                None => missing.push(name.to_string()),
            }
        }
    }
    let mut descriptions = Vec::new();
    if !missing.is_empty() {
        descriptions.push(format!("缺少 {}", missing.join("、")));
    }
    if !unavailable.is_empty() {
        descriptions.push(format!("不可用 {}", unavailable.join("、")));
    }
    if descriptions.is_empty() {
        "自动编队失败：缺少干员".to_string()
    } else {
        format!("自动编队失败：{}", descriptions.join("；"))
    }
}

impl CoreLogCursor {
    pub(super) fn prepare(track_daily: bool, track_copilot: bool) -> Result<Self, String> {
        let dir = maa_log_dir().map_err(|error| format!("无法定位 MaaCore 日志目录: {error}"))?;
        Self::prepare_in_dir(&dir, track_daily, track_copilot)
    }

    pub(super) fn prepare_in_dir(
        dir: &Path,
        track_daily: bool,
        track_copilot: bool,
    ) -> Result<Self, String> {
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
            track_daily,
            track_copilot,
        })
    }

    pub(super) fn poll(&mut self) -> std::io::Result<Vec<CoreProgressEvent>> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(error),
        };
        let current_identity = FileIdentity::from_metadata(&metadata);
        let mut events = Vec::new();
        if self
            .identity
            .is_some_and(|identity| identity != current_identity)
            || metadata.len() < self.offset
        {
            events.extend(self.read_rotated_backup()?);
            self.offset = 0;
            self.partial.clear();
        }
        self.identity = Some(current_identity);
        events.extend(self.read_current()?);
        Ok(events)
    }

    fn read_rotated_backup(&mut self) -> std::io::Result<Vec<CoreProgressEvent>> {
        let Some(identity) = self.identity else {
            return Ok(Vec::new());
        };
        let Ok(metadata) = fs::metadata(&self.backup_path) else {
            return Ok(Vec::new());
        };
        if FileIdentity::from_metadata(&metadata) != identity || metadata.len() <= self.offset {
            return Ok(Vec::new());
        }
        let mut lines = Vec::new();
        self.read_from(self.backup_path.clone(), metadata.len(), &mut lines)?;
        Ok(lines
            .into_iter()
            .filter_map(|line| self.parse_progress_line(&line))
            .collect())
    }

    fn read_current(&mut self) -> std::io::Result<Vec<CoreProgressEvent>> {
        let len = fs::metadata(&self.path)?.len();
        let mut lines = Vec::new();
        self.read_from(self.path.clone(), len, &mut lines)?;
        Ok(lines
            .into_iter()
            .filter_map(|line| self.parse_progress_line(&line))
            .collect())
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

    pub(super) fn parse_progress_line(&self, line: &str) -> Option<CoreProgressEvent> {
        if !line.contains(&format!("[Px{}]", self.pid)) {
            return None;
        }
        let (callback, raw_json) = line
            .split_once("Assistant::append_callback | ")?
            .1
            .split_once(' ')?;
        let payload: JsonValue = serde_json::from_str(raw_json).ok()?;
        if self.track_daily && callback == "AllTasksCompleted" {
            return Some(CoreProgressEvent::DailyTasksCompleted);
        }
        let taskchain = payload.get("taskchain")?.as_str()?;
        if !((self.track_daily && taskchain != "Copilot")
            || (self.track_copilot && taskchain == "Copilot"))
        {
            return None;
        }
        if self.track_copilot
            && callback == "SubTaskStart"
            && taskchain == "Copilot"
            && payload
                .pointer("/details/task")
                .and_then(JsonValue::as_str)
                .is_some_and(|task| {
                    matches!(task, "StageDrops-Stars-3" | "StageDrops-Stars-Adverse")
                })
        {
            return Some(CoreProgressEvent::CopilotStageSucceeded);
        }
        let task_id = usize::try_from(payload.get("taskid")?.as_u64()?).ok()?;
        let taskchain = taskchain.to_string();
        match callback {
            "TaskChainStart" if self.track_daily => {
                Some(CoreProgressEvent::DailyTaskStarted { task_id, taskchain })
            }
            "TaskChainStart" if self.track_copilot => {
                Some(CoreProgressEvent::CopilotTaskStarted { task_id })
            }
            "TaskChainCompleted" => {
                Some(CoreProgressEvent::TaskChainSucceeded { task_id, taskchain })
            }
            "TaskChainError" => Some(CoreProgressEvent::TaskChainFailed {
                task_id,
                taskchain,
                error: payload
                    .pointer("/details/error")
                    .and_then(JsonValue::as_str)
                    .filter(|error| !error.is_empty())
                    .map(str::to_string),
            }),
            "SubTaskError" => {
                let why = payload
                    .get("why")
                    .and_then(JsonValue::as_str)
                    .filter(|value| !value.is_empty());
                let what = payload
                    .get("what")
                    .and_then(JsonValue::as_str)
                    .filter(|value| !value.is_empty());
                let subtask = payload.get("subtask").and_then(JsonValue::as_str);
                let first = payload
                    .pointer("/details/first")
                    .and_then(JsonValue::as_array)
                    .and_then(|items| items.first())
                    .and_then(JsonValue::as_str)
                    .filter(|value| !value.is_empty());
                let reason = if taskchain == "Copilot" && why == Some("OperatorMissing") {
                    missing_operators(&payload)
                } else if subtask == Some("StageNavigationTask") {
                    first.map_or_else(
                        || "关卡导航失败".to_string(),
                        |first| format!("关卡导航失败（{first}）"),
                    )
                } else if let Some(reason) = why.or(what) {
                    reason.to_string()
                } else if subtask == Some("ProcessTask") {
                    first.map(|first| format!("关卡导航失败（{first}）"))?
                } else {
                    return None;
                };
                Some(CoreProgressEvent::SubTaskFailure {
                    task_id,
                    taskchain,
                    reason,
                })
            }
            _ => None,
        }
    }
}
