//! Stage 目录数据模型。

use crate::stage::refresh::{cache_paths, refresh_catalog};
use crate::stage::time::{availability, now_epoch, parse_timestamp, parse_timezone_offset};
use serde_json::Value;
use std::fs;
use std::sync::mpsc::Sender;
use std::thread;

#[derive(Debug, Clone)]
pub struct StageEntry {
    pub client: String,
    pub value: String,
    pub label: String,
    pub description: String,
    pub available: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StageCatalog {
    entries: Vec<StageEntry>,
}

#[derive(Debug)]
pub enum StageRefreshEvent {
    Updated {
        catalog: StageCatalog,
        tasks_updated: bool,
    },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Availability {
    Open,
    Upcoming,
    Expired,
    Unknown,
}

impl Availability {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "开放中",
            Self::Upcoming => "未开放",
            Self::Expired => "已结束",
            Self::Unknown => "状态未知",
        }
    }
}
impl StageCatalog {
    pub fn load_cached() -> Self {
        let Ok((_, path, _)) = cache_paths() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(json) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        Self::from_json(&json)
    }

    pub fn entries_for(&self, client: &str) -> Vec<StageEntry> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.client == client)
            .cloned()
            .collect();
        entries.sort_by_key(|entry| (!entry.available, entry.label.clone()));
        entries
    }

    pub fn spawn_refresh(sender: Sender<StageRefreshEvent>) -> std::io::Result<()> {
        thread::Builder::new()
            .name("maatui-stage-refresh".to_string())
            .spawn(move || match refresh_catalog() {
                Ok((catalog, tasks_updated)) => {
                    let _ = sender.send(StageRefreshEvent::Updated {
                        catalog,
                        tasks_updated,
                    });
                }
                Err(error) => {
                    let _ = sender.send(StageRefreshEvent::Failed(error.to_string()));
                }
            })
            .map(|_| ())
    }

    pub(super) fn from_json(root: &Value) -> Self {
        Self::from_json_at(root, now_epoch())
    }

    pub(super) fn from_json_at(root: &Value, now: i64) -> Self {
        let mut entries = Vec::new();
        let Some(clients) = root.as_object() else {
            return Self::default();
        };

        for (client, client_value) in clients {
            let Some(side_story) = client_value
                .get("sideStoryStage")
                .and_then(Value::as_object)
            else {
                continue;
            };
            for activity_value in side_story.values() {
                let activity = activity_value.get("Activity");
                let stage_name = activity
                    .and_then(|value| value.get("StageName"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tip = activity
                    .and_then(|value| value.get("Tip"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let timezone_offset = activity
                    .and_then(|value| value.get("TimeZone"))
                    .and_then(parse_timezone_offset)
                    .unwrap_or(0);
                let start = activity
                    .and_then(|value| value.get("UtcStartTime"))
                    .and_then(Value::as_str)
                    .and_then(|value| parse_timestamp(value, timezone_offset));
                let expire = activity
                    .and_then(|value| value.get("UtcExpireTime"))
                    .and_then(Value::as_str)
                    .and_then(|value| parse_timestamp(value, timezone_offset));
                let status = availability(start, expire, now);
                let Some(stages) = activity_value.get("Stages").and_then(Value::as_array) else {
                    continue;
                };
                for stage in stages {
                    let Some(value) = stage.get("Value").and_then(Value::as_str) else {
                        continue;
                    };
                    let display = stage
                        .get("Display")
                        .and_then(Value::as_str)
                        .unwrap_or(value);
                    let drop = stage
                        .get("Drop")
                        .and_then(Value::as_str)
                        .filter(|drop| !drop.is_empty());
                    let minimum = stage
                        .get("MinimumRequired")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            activity_value
                                .get("MinimumRequired")
                                .and_then(Value::as_str)
                        });
                    let activity_label = if stage_name.is_empty() {
                        tip.to_string()
                    } else if tip.is_empty() {
                        stage_name.to_string()
                    } else {
                        format!("{stage_name} · {tip}")
                    };
                    let label = if activity_label.is_empty() {
                        format!("{display} [{}]", status.label())
                    } else {
                        format!("{display} · {activity_label} [{}]", status.label())
                    };
                    let mut description = Vec::new();
                    if let Some(drop) = drop {
                        description.push(format!("掉落：{drop}"));
                    }
                    description.push(format!("活动状态：{}", status.label()));
                    if let Some(minimum) = minimum {
                        description.push(format!("最低资源版本：{minimum}"));
                    }
                    entries.push(StageEntry {
                        client: client.clone(),
                        value: value.to_string(),
                        label,
                        description: description.join(" · "),
                        available: status == Availability::Open,
                    });
                }
            }
        }

        Self { entries }
    }
}
