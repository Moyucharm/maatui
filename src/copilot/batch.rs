//! 批量作业任务文件生成与清理。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Value as JsonValue, json};

use super::cache::{CopilotCache, CopilotEntry, CopilotEntrySource};
use crate::storage::{atomic_write, maatui_cache_dir};

#[derive(Debug, Clone)]
pub struct CopilotRunOptions {
    pub formation_index: i64,
    pub use_sanity_potion: bool,
    pub add_trust: bool,
    pub ignore_requirements: bool,
    pub support_unit_usage: i64,
    pub support_unit_name: String,
}

#[derive(Debug)]
pub struct BatchTask {
    pub path: PathBuf,
    pub indices: Vec<usize>,
}

pub fn write_batch_task(
    cache: &CopilotCache,
    indices: Vec<usize>,
    options: &CopilotRunOptions,
) -> Result<BatchTask> {
    write_batch_task_in(cache, indices, options, &maatui_cache_dir()?)
}

pub(crate) fn write_batch_task_in(
    cache: &CopilotCache,
    indices: Vec<usize>,
    options: &CopilotRunOptions,
    cache_dir: &Path,
) -> Result<BatchTask> {
    if indices.is_empty() {
        bail!("没有启用的作业");
    }
    let mut list = Vec::with_capacity(indices.len());
    for &index in &indices {
        let entry = cache.entry(index).context("作业索引超出范围")?;
        if entry.stage_name.trim().is_empty() {
            bail!("作业 {} 缺少关卡名", index + 1);
        }
        let path = cache
            .resolved_file_path(index)
            .context("无法解析作业文件路径")?;
        if !path.is_file() {
            bail!("作业文件不存在: {}", path.display());
        }
        list.push(json!({
            "filename": path,
            "stage_name": entry.stage_name,
            "is_raid": entry.is_raid,
        }));
    }

    let mut params = serde_json::Map::new();
    params.insert("copilot_list".to_string(), JsonValue::Array(list));
    params.insert("formation".to_string(), JsonValue::Bool(true));
    params.insert(
        "use_sanity_potion".to_string(),
        JsonValue::Bool(options.use_sanity_potion),
    );
    params.insert("add_trust".to_string(), JsonValue::Bool(options.add_trust));
    params.insert(
        "ignore_requirements".to_string(),
        JsonValue::Bool(options.ignore_requirements),
    );
    if options.formation_index > 0 {
        params.insert(
            "formation_index".to_string(),
            options.formation_index.into(),
        );
    }
    if options.support_unit_usage > 0 {
        params.insert(
            "support_unit_usage".to_string(),
            options.support_unit_usage.into(),
        );
    }
    if !options.support_unit_name.trim().is_empty() {
        params.insert(
            "support_unit_name".to_string(),
            JsonValue::String(options.support_unit_name.trim().to_string()),
        );
    }

    let task = json!({
        "tasks": [{
            "name": "作业集自动战斗",
            "type": "Copilot",
            "params": params,
        }]
    });
    static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);
    let sequence = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = cache_dir.join("copilot-runs").join(format!(
        "copilot-batch-{}-{sequence}.json",
        std::process::id()
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建批量任务目录失败: {}", parent.display()))?;
    }
    let content = format!("{}\n", serde_json::to_string_pretty(&task)?);
    atomic_write(&path, content.as_bytes())?;
    Ok(BatchTask { path, indices })
}

pub fn remove_batch_task(path: &Path) {
    let _ = fs::remove_file(path);
}

pub(super) fn resolve_entry_path(entry: &CopilotEntry, files_dir: &Path) -> PathBuf {
    match &entry.source {
        CopilotEntrySource::Remote { id } => files_dir.join(format!("{id}.json")),
        CopilotEntrySource::Local { path } => path.clone(),
    }
}
