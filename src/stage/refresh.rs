//! Stage 网络刷新与缓存。

use super::model::StageCatalog;
use crate::http::{USER_AGENT, agent};
use crate::storage::{atomic_write, maa_config_dir};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const STAGE_ACTIVITY_URLS: [&str; 2] = [
    "https://api.maa.plus/MaaAssistantArknights/api/gui/StageActivityV2.json",
    "https://ota.maa.plus/MaaAssistantArknights/api/gui/StageActivityV2.json",
];
const TASKS_URLS: [&str; 2] = [
    "https://api.maa.plus/MaaAssistantArknights/api/resource/tasks.json",
    "https://ota.maa.plus/MaaAssistantArknights/api/resource/tasks.json",
];

pub(super) fn refresh_catalog() -> Result<(StageCatalog, bool)> {
    let (trusted_root, stage_path, tasks_path) = cache_paths()?;
    let agent = agent();
    let stage_json = fetch_json(&agent, &STAGE_ACTIVITY_URLS, &stage_path, &trusted_root)?;
    let tasks_updated = fetch_json(&agent, &TASKS_URLS, &tasks_path, &trusted_root).is_ok();
    Ok((StageCatalog::from_json(&stage_json), tasks_updated))
}

pub(super) fn fetch_json(
    agent: &ureq::Agent,
    urls: &[&str],
    cache_path: &Path,
    trusted_root: &Path,
) -> Result<Value> {
    let etag_path = PathBuf::from(format!("{}.etag", cache_path.display()));
    let etag = fs::read_to_string(&etag_path).unwrap_or_default();
    let mut errors = Vec::new();

    for url in urls {
        let mut request = agent.get(url).set("User-Agent", USER_AGENT);
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
                atomic_write(cache_path, trusted_root, raw.as_bytes())?;
                if !next_etag.is_empty() {
                    atomic_write(&etag_path, trusted_root, next_etag.as_bytes())?;
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

pub(super) fn read_cached_json(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取热更新缓存失败: {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("解析热更新缓存失败: {}", path.display()))
}

pub(super) fn cache_paths() -> Result<(PathBuf, PathBuf, PathBuf)> {
    let root = maa_config_dir()?;
    Ok((
        root.clone(),
        root.join("cache/gui/StageActivityV2.json"),
        root.join("cache/resource/tasks.json"),
    ))
}
