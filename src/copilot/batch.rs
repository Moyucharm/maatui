//! 批量作业任务文件生成与清理。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Value as JsonValue, json};

use super::cache::{CopilotCache, CopilotEntry, CopilotEntrySource};
use crate::storage::{atomic_write, maatui_cache_dir};
use crate::tile_alias::StageAliasIndex;

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
    let alias_index = StageAliasIndex::load();
    write_batch_task_in_with_resolver(cache, indices, options, cache_dir, |stage| {
        alias_index.resolve(stage)
    })
}

fn write_batch_task_in_with_resolver(
    cache: &CopilotCache,
    indices: Vec<usize>,
    options: &CopilotRunOptions,
    cache_dir: &Path,
    resolve_stage_code: impl Fn(&str) -> Option<String>,
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
        let stage_name =
            resolve_stage_code(&entry.stage_name).unwrap_or_else(|| entry.stage_name.clone());
        list.push(json!({
            "filename": path,
            "stage_name": stage_name,
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
    atomic_write(&path, cache_dir, content.as_bytes())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::CopilotOrigin;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maatui-copilot-batch-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normalizes_cached_internal_stage_name_when_writing_batch_task() {
        let dir = temp_dir();
        let files_dir = dir.join("files");
        fs::create_dir_all(&files_dir).unwrap();
        fs::write(files_dir.join("99366.json"), "{}").unwrap();

        let mut cache =
            CopilotCache::load(dir.join("copilot-set.json"), files_dir.clone()).unwrap();
        cache
            .append(vec![CopilotEntry {
                enabled: true,
                stage_name: "act53side_ex01".to_string(),
                title: Some("TO-EX-1 测试作业".to_string()),
                is_raid: false,
                source: CopilotEntrySource::Remote { id: 99366 },
                origin: CopilotOrigin::Set {
                    id: 51251,
                    name: Some("测试作业集".to_string()),
                },
            }])
            .unwrap();

        let task = write_batch_task_in_with_resolver(
            &cache,
            vec![0],
            &CopilotRunOptions {
                formation_index: 0,
                use_sanity_potion: false,
                add_trust: true,
                ignore_requirements: true,
                support_unit_usage: 0,
                support_unit_name: String::new(),
            },
            &dir,
            |stage_name| (stage_name == "act53side_ex01").then(|| "TO-EX-1".to_string()),
        )
        .unwrap();

        let value: JsonValue =
            serde_json::from_str(&fs::read_to_string(&task.path).unwrap()).unwrap();
        let item = value.pointer("/tasks/0/params/copilot_list/0").unwrap();
        assert_eq!(item["stage_name"], "TO-EX-1");
        assert_eq!(item["is_raid"], false);
        assert_eq!(
            item["filename"].as_str(),
            files_dir.join("99366.json").to_str()
        );
        assert_eq!(task.indices, vec![0]);

        fs::remove_dir_all(dir).unwrap();
    }
}
