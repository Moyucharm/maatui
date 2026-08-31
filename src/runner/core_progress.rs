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
    DailyTaskStarted { task_id: usize, taskchain: String },
    CopilotStageSucceeded,
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
        let taskchain = payload.get("taskchain")?.as_str()?;

        if self.track_daily && callback == "TaskChainStart" {
            let task_id = payload.get("taskid")?.as_u64()? as usize;
            return Some(CoreProgressEvent::DailyTaskStarted {
                task_id,
                taskchain: taskchain.to_string(),
            });
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
        None
    }
}
