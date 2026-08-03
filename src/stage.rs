//! MAA 活动关卡目录：缓存、热更新与活动状态解析。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::storage::maa_config_dir;

const STAGE_ACTIVITY_URLS: [&str; 2] = [
    "https://api.maa.plus/MaaAssistantArknights/api/gui/StageActivityV2.json",
    "https://ota.maa.plus/MaaAssistantArknights/api/gui/StageActivityV2.json",
];
const TASKS_URLS: [&str; 2] = [
    "https://api.maa.plus/MaaAssistantArknights/api/resource/tasks.json",
    "https://ota.maa.plus/MaaAssistantArknights/api/resource/tasks.json",
];
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

impl StageCatalog {
    pub fn load_cached() -> Self {
        let Ok((path, _)) = cache_paths() else {
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

    pub fn spawn_refresh(sender: Sender<StageRefreshEvent>) {
        let _ = thread::Builder::new()
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
            });
    }

    fn from_json(root: &Value) -> Self {
        Self::from_json_at(root, now_epoch())
    }

    fn from_json_at(root: &Value, now: i64) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Availability {
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

fn refresh_catalog() -> Result<(StageCatalog, bool)> {
    let (stage_path, tasks_path) = cache_paths()?;
    let agent = http_agent();
    let stage_json = fetch_json(&agent, &STAGE_ACTIVITY_URLS, &stage_path)?;
    let tasks_updated = fetch_json(&agent, &TASKS_URLS, &tasks_path).is_ok();
    Ok((StageCatalog::from_json(&stage_json), tasks_updated))
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(REQUEST_TIMEOUT)
        .timeout_write(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .build()
}

fn fetch_json(agent: &ureq::Agent, urls: &[&str], cache_path: &Path) -> Result<Value> {
    let etag_path = PathBuf::from(format!("{}.etag", cache_path.display()));
    let etag = fs::read_to_string(&etag_path).unwrap_or_default();
    let mut errors = Vec::new();

    for url in urls {
        let mut request = agent.get(url).set("User-Agent", "MaaTUI/0.1");
        if !etag.trim().is_empty() {
            request = request.set("If-None-Match", etag.trim());
        }
        match request.call() {
            Ok(response) => {
                if response.status() == 304 {
                    return read_cached_json(cache_path);
                }
                let next_etag = response.header("ETag").unwrap_or_default().to_string();
                let raw = response
                    .into_string()
                    .with_context(|| format!("读取热更新响应失败: {url}"))?;
                let json: Value = serde_json::from_str(&raw)
                    .with_context(|| format!("解析热更新 JSON 失败: {url}"))?;
                if let Some(parent) = cache_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(cache_path, raw)?;
                if !next_etag.is_empty() {
                    fs::write(&etag_path, next_etag)?;
                }
                return Ok(json);
            }
            Err(ureq::Error::Status(304, _)) => return read_cached_json(cache_path),
            Err(error) => errors.push(format!("{url}: {error}")),
        }
    }

    read_cached_json(cache_path).with_context(|| {
        if errors.is_empty() {
            "热更新请求失败，且没有本地缓存".to_string()
        } else {
            format!("热更新请求失败：{}", errors.join("；"))
        }
    })
}

fn read_cached_json(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取热更新缓存失败: {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("解析热更新缓存失败: {}", path.display()))
}

fn cache_paths() -> Result<(PathBuf, PathBuf)> {
    let root = maa_config_dir()?;
    Ok((
        root.join("cache/gui/StageActivityV2.json"),
        root.join("cache/resource/tasks.json"),
    ))
}

fn availability(start: Option<i64>, expire: Option<i64>, now: i64) -> Availability {
    match (start, expire) {
        (Some(start), Some(_expire)) if now < start => Availability::Upcoming,
        (Some(_), Some(expire)) if now <= expire => Availability::Open,
        (Some(_), Some(_)) => Availability::Expired,
        _ => Availability::Unknown,
    }
}

fn parse_timezone_offset(value: &Value) -> Option<i64> {
    let hours = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;
    if !hours.is_finite() || !(-24.0..=24.0).contains(&hours) {
        return None;
    }
    Some((hours * 3_600.0).round() as i64)
}

fn parse_timestamp(value: &str, timezone_offset: i64) -> Option<i64> {
    let mut parts = value.split(['/', ' ', ':']);
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    let hour = parts.next()?.parse::<i64>().ok()?;
    let minute = parts.next()?.parse::<i64>().ok()?;
    let second = parts.next()?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
            - timezone_offset,
    )
}

// Howard Hinnant 的 civil date 算法：不引入重量级日期依赖即可比较 UTC 时间。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stage_activity_entries() {
        let json = serde_json::json!({
            "Official": {
                "sideStoryStage": {
                    "TEST": {
                        "MinimumRequired": "v6.0.0",
                        "Activity": {
                            "Tip": "测试活动",
                            "StageName": "TEST",
                            "UtcStartTime": "2099/01/01 00:00:00",
                            "UtcExpireTime": "2099/01/02 00:00:00",
                            "TimeZone": 8
                        },
                        "Stages": [{
                            "Display": "TEST-8",
                            "Value": "TEST-8",
                            "Drop": "测试材料"
                        }]
                    }
                }
            }
        });
        let catalog = StageCatalog::from_json(&json);
        let entries = catalog.entries_for("Official");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].value, "TEST-8");
        assert!(entries[0].label.contains("TEST-8"));
        assert!(entries[0].description.contains("测试材料"));
    }

    #[test]
    fn parses_activity_timestamp_with_timezone_offset() {
        assert_eq!(parse_timestamp("1970/01/01 00:00:00", 0), Some(0));
        assert_eq!(parse_timestamp("1970/01/01 08:00:00", 8 * 3_600), Some(0));
        assert_eq!(parse_timestamp("1969/12/31 19:00:00", -5 * 3_600), Some(0));
        assert!(parse_timestamp("not-a-date", 0).is_none());
    }

    #[test]
    fn parses_numeric_and_string_timezones_safely() {
        assert_eq!(parse_timezone_offset(&serde_json::json!(8)), Some(28_800));
        assert_eq!(
            parse_timezone_offset(&serde_json::json!("-5.5")),
            Some(-19_800)
        );
        assert_eq!(parse_timezone_offset(&serde_json::json!(25)), None);
        assert_eq!(parse_timezone_offset(&serde_json::json!("invalid")), None);
    }

    #[test]
    fn applies_timezone_before_calculating_activity_status() {
        let json = serde_json::json!({
            "Official": {
                "sideStoryStage": {
                    "TEST": {
                        "Activity": {
                            "UtcStartTime": "1970/01/01 08:00:00",
                            "UtcExpireTime": "1970/01/01 09:00:00",
                            "TimeZone": 8
                        },
                        "Stages": [{"Display": "TEST-1", "Value": "TEST-1"}]
                    }
                }
            }
        });

        let catalog = StageCatalog::from_json_at(&json, 30 * 60);
        let entries = catalog.entries_for("Official");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].available);
        assert!(entries[0].label.contains("开放中"));
    }

    #[test]
    fn handles_activity_availability_boundaries() {
        assert_eq!(
            availability(Some(100), Some(200), 99),
            Availability::Upcoming
        );
        assert_eq!(availability(Some(100), Some(200), 100), Availability::Open);
        assert_eq!(availability(Some(100), Some(200), 200), Availability::Open);
        assert_eq!(
            availability(Some(100), Some(200), 201),
            Availability::Expired
        );
        assert_eq!(availability(None, Some(200), 100), Availability::Unknown);
    }

    #[test]
    fn keeps_client_entries_separate() {
        let json = serde_json::json!({
            "Official": {"sideStoryStage": {}},
            "Bilibili": {"sideStoryStage": {}}
        });
        let catalog = StageCatalog::from_json(&json);
        assert!(catalog.entries_for("Official").is_empty());
        assert!(catalog.entries_for("Bilibili").is_empty());
    }
}
