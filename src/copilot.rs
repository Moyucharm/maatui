//! 自动战斗作业列表：持久化、远程导入与批量任务生成。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::storage::{atomic_write, maa_config_dir, maatui_cache_dir};
use crate::tile_alias;

const CACHE_VERSION: u32 = 4;
const COPILOT_API: &str = "https://prts.maa.plus/copilot/get/";
const COPILOT_SET_API: &str = "https://prts.maa.plus/set/get?id=";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CopilotEntrySource {
    Remote { id: u64 },
    Local { path: PathBuf },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CopilotOrigin {
    #[default]
    Single,
    Set {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl CopilotOrigin {
    pub fn is_set(&self) -> bool {
        matches!(self, Self::Set { .. })
    }

    pub fn label(&self) -> String {
        match self {
            Self::Single => "单个添加".to_string(),
            Self::Set { id, name } => name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| format!("作业集 #{id}"), |name| name.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopilotEntry {
    pub enabled: bool,
    pub stage_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub is_raid: bool,
    pub source: CopilotEntrySource,
    #[serde(default)]
    pub origin: CopilotOrigin,
}

impl CopilotEntry {
    pub fn source_label(&self) -> String {
        match &self.source {
            CopilotEntrySource::Remote { id } => format!("#{id}"),
            CopilotEntrySource::Local { path } => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("本地")
                .to_string(),
        }
    }

    pub fn display_name(&self) -> String {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&self.stage_name)
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopilotOptions {
    #[serde(default)]
    pub formation: bool,
    #[serde(default)]
    pub formation_index: i64,
    #[serde(default)]
    pub use_sanity_potion: bool,
    #[serde(default)]
    pub add_trust: bool,
    #[serde(default)]
    pub ignore_requirements: bool,
    #[serde(default)]
    pub support_unit_usage: i64,
    #[serde(default)]
    pub support_unit_name: String,
    #[serde(default = "default_loop_times")]
    pub loop_times: i64,
}

const fn default_loop_times() -> i64 {
    1
}

impl Default for CopilotOptions {
    fn default() -> Self {
        Self {
            formation: false,
            formation_index: 0,
            use_sanity_potion: false,
            add_trust: false,
            ignore_requirements: false,
            support_unit_usage: 0,
            support_unit_name: String::new(),
            loop_times: default_loop_times(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CopilotCacheFile {
    version: u32,
    #[serde(default)]
    settings: CopilotOptions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_single: Option<CopilotEntry>,
    #[serde(default)]
    entries: Vec<CopilotEntry>,
}

pub struct CopilotCache {
    path: PathBuf,
    files_dir: PathBuf,
    settings: CopilotOptions,
    current_single: Option<CopilotEntry>,
    entries: Vec<CopilotEntry>,
    migrated_single_reset: bool,
}

impl CopilotCache {
    pub fn load_default() -> Result<Self> {
        let root = maa_config_dir()?.join("maatui");
        Self::load(root.join("copilot-set.json"), root.join("copilot"))
    }

    pub fn load(path: PathBuf, files_dir: PathBuf) -> Result<Self> {
        let (settings, current_single, entries, migrated_single_reset, needs_save) =
            if path.is_file() {
                let raw = fs::read_to_string(&path)
                    .with_context(|| format!("读取作业列表失败: {}", path.display()))?;
                let file: CopilotCacheFile = serde_json::from_str(&raw)
                    .with_context(|| format!("解析作业列表失败: {}", path.display()))?;
                if !(1..=CACHE_VERSION).contains(&file.version) {
                    bail!("不支持的作业列表版本: {}", file.version);
                }
                let (current_single, entries, migrated_single_reset) = if file.version < 3 {
                    let had_single = file.entries.iter().any(|entry| !entry.origin.is_set());
                    let entries = file
                        .entries
                        .into_iter()
                        .filter(|entry| entry.origin.is_set())
                        .collect();
                    (None, entries, had_single)
                } else {
                    (file.current_single, file.entries, false)
                };
                (
                    file.settings,
                    current_single,
                    entries,
                    migrated_single_reset,
                    file.version < CACHE_VERSION,
                )
            } else {
                (CopilotOptions::default(), None, Vec::new(), false, false)
            };
        let cache = Self {
            path,
            files_dir,
            settings,
            current_single,
            entries,
            migrated_single_reset,
        };
        if needs_save {
            cache.save().context("保存升级后的作业缓存失败")?;
        }
        Ok(cache)
    }

    pub fn settings(&self) -> &CopilotOptions {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: CopilotOptions) -> Result<()> {
        let original = std::mem::replace(&mut self.settings, settings);
        if let Err(error) = self.save() {
            self.settings = original;
            return Err(error);
        }
        Ok(())
    }

    pub fn files_dir(&self) -> &Path {
        &self.files_dir
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[CopilotEntry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current_single(&self) -> Option<&CopilotEntry> {
        self.current_single.as_ref()
    }

    pub fn current_single_path(&self) -> Option<PathBuf> {
        self.current_single
            .as_ref()
            .map(|entry| resolve_entry_path(entry, &self.files_dir))
    }

    pub fn replace_current_single(&mut self, entry: Option<CopilotEntry>) -> Result<()> {
        let original = self.current_single.clone();
        self.current_single = entry;
        if let Err(error) = self.save() {
            self.current_single = original;
            return Err(error);
        }
        Ok(())
    }

    pub fn current_single_supported_modes(&self) -> Result<(bool, bool)> {
        let entry = self.current_single().context("请先搜索当前单作业")?;
        let path = self.current_single_path().context("无法解析作业文件路径")?;
        supported_modes_from_path(entry, &path)
    }

    pub fn toggle_current_single_raid(&mut self) -> Result<bool> {
        let (supports_normal, supports_raid) = self.current_single_supported_modes()?;
        if !(supports_normal && supports_raid) {
            bail!("当前作业只支持一种难度，无法切换");
        }
        let original = self.current_single.clone();
        let entry = self.current_single.as_mut().context("请先搜索当前单作业")?;
        entry.is_raid = !entry.is_raid;
        let is_raid = entry.is_raid;
        if let Err(error) = self.save() {
            self.current_single = original;
            return Err(error);
        }
        Ok(is_raid)
    }

    pub fn enabled_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.enabled).count()
    }

    pub fn entries_len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn migrated_single_reset_for_test(&self) -> bool {
        self.migrated_single_reset
    }

    #[cfg(test)]
    pub fn indices_for_origin(&self, is_set: bool) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (entry.origin.is_set() == is_set).then_some(index))
            .collect()
    }

    pub fn migrated_single_reset(&self) -> bool {
        self.migrated_single_reset
    }

    pub fn entry(&self, index: usize) -> Option<&CopilotEntry> {
        self.entries.get(index)
    }

    pub fn resolved_file_path(&self, index: usize) -> Option<PathBuf> {
        self.entries
            .get(index)
            .map(|entry| resolve_entry_path(entry, &self.files_dir))
    }

    pub fn enabled_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.enabled.then_some(index))
            .collect()
    }

    pub fn append(&mut self, entries: Vec<CopilotEntry>) -> Result<usize> {
        if entries.is_empty() {
            return Ok(self.entries.len());
        }
        self.mutate_and_save(|current| {
            let first = current.len();
            current.extend(entries);
            Ok(first)
        })
    }

    pub fn toggle(&mut self, index: usize) -> Result<bool> {
        self.mutate_and_save(|entries| {
            let entry = entries.get_mut(index).context("作业索引超出范围")?;
            entry.enabled = !entry.enabled;
            Ok(entry.enabled)
        })
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) -> Result<()> {
        self.mutate_and_save(|entries| {
            let entry = entries.get_mut(index).context("作业索引超出范围")?;
            entry.enabled = enabled;
            Ok(())
        })
    }

    pub fn set_enabled_many(&mut self, indices: &[usize], enabled: bool) -> Result<()> {
        self.mutate_and_save(|entries| {
            for &index in indices {
                let entry = entries.get_mut(index).context("作业索引超出范围")?;
                entry.enabled = enabled;
            }
            Ok(())
        })
    }

    pub fn set_all_enabled(&mut self, enabled: bool) -> Result<()> {
        self.mutate_and_save(|entries| {
            for entry in entries {
                entry.enabled = enabled;
            }
            Ok(())
        })
    }

    pub fn clear_entries(&mut self) -> Result<()> {
        self.mutate_and_save(|entries| {
            entries.clear();
            Ok(())
        })
    }

    pub fn delete(&mut self, index: usize) -> Result<()> {
        self.mutate_and_save(|entries| {
            if index >= entries.len() {
                bail!("作业索引超出范围");
            }
            entries.remove(index);
            Ok(())
        })
    }

    pub fn swap_entries(&mut self, first: usize, second: usize) -> Result<()> {
        if first == second {
            return Ok(());
        }
        self.mutate_and_save(|entries| {
            if first >= entries.len() || second >= entries.len() {
                bail!("作业索引超出范围");
            }
            entries.swap(first, second);
            Ok(())
        })
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建作业列表目录失败: {}", parent.display()))?;
        }
        let file = CopilotCacheFile {
            version: CACHE_VERSION,
            settings: self.settings.clone(),
            current_single: self.current_single.clone(),
            entries: self.entries.clone(),
        };
        let content = format!("{}\n", serde_json::to_string_pretty(&file)?);
        atomic_write(&self.path, content.as_bytes())
    }

    fn mutate_and_save<T>(
        &mut self,
        operation: impl FnOnce(&mut Vec<CopilotEntry>) -> Result<T>,
    ) -> Result<T> {
        let original = self.entries.clone();
        let value = match operation(&mut self.entries) {
            Ok(value) => value,
            Err(error) => {
                self.entries = original;
                return Err(error);
            }
        };
        if let Err(error) = self.save() {
            self.entries = original;
            return Err(error);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Single,
    Set,
}

#[derive(Debug, Clone)]
pub struct ImportProgress {
    pub completed: usize,
    pub total: usize,
    pub label: String,
}

#[derive(Debug)]
pub struct ImportReport {
    pub entries: Vec<CopilotEntry>,
    pub errors: Vec<String>,
    pub set_id: Option<u64>,
    pub set_name: Option<String>,
    pub set_description: Option<String>,
}

impl ImportReport {
    fn single(entries: Vec<CopilotEntry>) -> Self {
        Self {
            entries,
            errors: Vec::new(),
            set_id: None,
            set_name: None,
            set_description: None,
        }
    }
}

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

pub fn import_source(
    input: &str,
    kind: ImportKind,
    files_dir: &Path,
    progress: impl FnMut(ImportProgress),
) -> Result<ImportReport> {
    let agent = copilot_agent();
    import_source_with_agent(input, kind, files_dir, progress, &agent)
}

fn import_source_with_agent(
    input: &str,
    kind: ImportKind,
    files_dir: &Path,
    mut progress: impl FnMut(ImportProgress),
    agent: &ureq::Agent,
) -> Result<ImportReport> {
    match kind {
        ImportKind::Single => match parse_single_source(input)? {
            ParsedSingleSource::Remote(id) => {
                progress(ImportProgress {
                    completed: 0,
                    total: 1,
                    label: format!("下载作业 #{id}"),
                });
                let parsed = fetch_remote_copilot(agent, id, files_dir)?;
                progress(ImportProgress {
                    completed: 1,
                    total: 1,
                    label: format!("已解析 {}", parsed.stage_name),
                });
                Ok(ImportReport::single(parsed.into_entries(
                    CopilotEntrySource::Remote { id },
                    CopilotOrigin::Single,
                )))
            }
            ParsedSingleSource::Local(path) => {
                let canonical = fs::canonicalize(&path)
                    .with_context(|| format!("定位本地作业失败: {}", path.display()))?;
                let raw = fs::read_to_string(&canonical)
                    .with_context(|| format!("读取本地作业失败: {}", canonical.display()))?;
                let parsed = parse_copilot_content(&serde_json::from_str(&raw)?)?;
                progress(ImportProgress {
                    completed: 1,
                    total: 1,
                    label: format!("已解析 {}", parsed.stage_name),
                });
                Ok(ImportReport::single(parsed.into_entries(
                    CopilotEntrySource::Local { path: canonical },
                    CopilotOrigin::Single,
                )))
            }
        },
        ImportKind::Set => {
            let id = parse_set_code(input)?;
            let set = fetch_copilot_set(agent, id)?;
            let total = set.copilot_ids.len();
            let origin = CopilotOrigin::Set {
                id,
                name: non_empty(set.name.clone()),
            };
            let mut entries = Vec::new();
            let mut errors = Vec::new();
            for (offset, copilot_id) in set.copilot_ids.iter().copied().enumerate() {
                progress(ImportProgress {
                    completed: offset,
                    total,
                    label: format!("下载作业 #{copilot_id}"),
                });
                match fetch_remote_copilot(agent, copilot_id, files_dir) {
                    Ok(parsed) => entries.extend(parsed.into_entries(
                        CopilotEntrySource::Remote { id: copilot_id },
                        origin.clone(),
                    )),
                    Err(error) => errors.push(format!("#{copilot_id}: {error:#}")),
                }
                progress(ImportProgress {
                    completed: offset + 1,
                    total,
                    label: format!("已处理 {}/{}", offset + 1, total),
                });
            }
            Ok(ImportReport {
                entries,
                errors,
                set_id: Some(id),
                set_name: non_empty(set.name),
                set_description: non_empty(set.description),
            })
        }
    }
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

pub fn resolve_entry_path(entry: &CopilotEntry, files_dir: &Path) -> PathBuf {
    match &entry.source {
        CopilotEntrySource::Remote { id } => files_dir.join(format!("{id}.json")),
        CopilotEntrySource::Local { path } => path.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct CopilotDetail {
    pub title: String,
    pub lines: Vec<String>,
}

impl CopilotCache {
    pub fn current_single_detail(&self) -> Result<CopilotDetail> {
        let entry = self.current_single().context("请先搜索当前单作业")?;
        let path = self.current_single_path().context("无法解析作业文件路径")?;
        detail_from_path(entry, &path)
    }

    pub fn detail(&self, index: usize) -> Result<CopilotDetail> {
        let entry = self.entry(index).context("作业索引超出范围")?;
        let path = self
            .resolved_file_path(index)
            .context("无法解析作业文件路径")?;
        detail_from_path(entry, &path)
    }
}

fn detail_from_path(entry: &CopilotEntry, path: &Path) -> Result<CopilotDetail> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取作业文件失败: {}", path.display()))?;
    let content: JsonValue = serde_json::from_str(&raw)
        .with_context(|| format!("解析作业文件失败: {}", path.display()))?;
    Ok(format_copilot_detail(entry, &content))
}

fn supported_modes_from_path(entry: &CopilotEntry, path: &Path) -> Result<(bool, bool)> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("读取作业文件失败: {}", path.display()))?;
    let content: JsonValue = serde_json::from_str(&raw)
        .with_context(|| format!("解析作业文件失败: {}", path.display()))?;
    let difficulty = content
        .get("difficulty")
        .and_then(JsonValue::as_i64)
        .unwrap_or(if entry.is_raid { 2 } else { 1 });
    if !(0..=3).contains(&difficulty) {
        bail!("作业 difficulty 必须为 0-3");
    }
    let difficulty = if difficulty == 0 { 1 } else { difficulty };
    Ok((difficulty & 1 != 0, difficulty & 2 != 0))
}

fn format_copilot_detail(entry: &CopilotEntry, content: &JsonValue) -> CopilotDetail {
    let mut lines = vec![
        format!("关卡名：{}", entry.stage_name),
        format!("难度：{}", if entry.is_raid { "突袭" } else { "普通" }),
        format!("来源：{} · {}", entry.origin.label(), entry.source_label()),
    ];
    if let Some(value) = content.get("minimum_required") {
        lines.push(format!("最低 Core 版本：{}", json_display(value)));
    }
    if let Some(value) = content.get("version") {
        lines.push(format!("作业版本：{}", json_display(value)));
    }

    lines.push(String::new());
    lines.push("需要的干员：".to_string());
    match content.get("opers").and_then(JsonValue::as_array) {
        Some(opers) if !opers.is_empty() => {
            for oper in opers {
                let name = oper
                    .get("name")
                    .map(json_display)
                    .unwrap_or_else(|| "未知干员".to_string());
                let skill = oper
                    .get("skill")
                    .map(json_display)
                    .map(|skill| format!("技能 {skill}"))
                    .unwrap_or_else(|| "技能未指定".to_string());
                let usage = oper
                    .get("skill_usage")
                    .map(json_display)
                    .map(|usage| format!("，模式 {usage}"))
                    .unwrap_or_default();
                let requirements = oper
                    .get("requirements")
                    .and_then(JsonValue::as_object)
                    .map(format_operator_requirements)
                    .unwrap_or_default();
                lines.push(format!("  · {name} · {skill}{usage}{requirements}"));
            }
        }
        _ => lines.push("  · 未提供干员需求".to_string()),
    }

    let skill_actions = content
        .get("actions")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter(|action| {
            matches!(
                action.get("type").and_then(JsonValue::as_str),
                Some("SkillUsage" | "Skill")
            )
        })
        .collect::<Vec<_>>();
    lines.push(String::new());
    lines.push("技能使用：".to_string());
    if skill_actions.is_empty() {
        lines.push("  · 未记录固定技能使用".to_string());
    } else {
        for action in skill_actions {
            let name = action
                .get("name")
                .map(json_display)
                .unwrap_or_else(|| "未知干员".to_string());
            let times = action
                .get("skill_times")
                .map(json_display)
                .map(|times| format!("，次数 {times}"))
                .unwrap_or_default();
            let trigger = [
                ("kills", "击杀"),
                ("costs", "费用"),
                ("pre_delay", "延迟(ms)"),
            ]
            .iter()
            .filter_map(|(key, label)| {
                action
                    .get(*key)
                    .map(|value| format!("，{label} {}", json_display(value)))
            })
            .collect::<String>();
            let note = action
                .get("doc")
                .and_then(JsonValue::as_str)
                .filter(|note| !note.trim().is_empty())
                .map(|note| format!("，备注 {note}"))
                .unwrap_or_default();
            let usage = action
                .get("skill_usage")
                .map(json_display)
                .map(|usage| format!("，模式 {usage}"))
                .unwrap_or_default();
            lines.push(format!("  · {name}{times}{usage}{trigger}{note}"));
        }
    }

    if let Some(groups) = content.get("groups").and_then(JsonValue::as_array)
        && !groups.is_empty()
    {
        lines.push(String::new());
        lines.push("作业分组：".to_string());
        for group in groups {
            let name = group
                .get("name")
                .map(json_display)
                .unwrap_or_else(|| "未命名分组".to_string());
            let opers = group
                .get("opers")
                .and_then(JsonValue::as_array)
                .map(|opers| {
                    opers
                        .iter()
                        .map(format_group_operator)
                        .collect::<Vec<_>>()
                        .join("；")
                })
                .filter(|opers| !opers.is_empty())
                .map(|opers| format!("：{opers}"))
                .unwrap_or_default();
            lines.push(format!("  · {name}{opers}"));
        }
    }

    lines.push(String::new());
    lines.push("备注：".to_string());
    let notes = content
        .pointer("/doc/details")
        .or_else(|| content.pointer("/doc/description"))
        .or_else(|| content.pointer("/documentation/details"))
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    if notes.trim().is_empty() {
        lines.push("  · 无".to_string());
    } else {
        lines.extend(notes.lines().map(|line| format!("  {line}")));
    }

    CopilotDetail {
        title: entry.display_name(),
        lines,
    }
}

fn format_group_operator(oper: &JsonValue) -> String {
    let Some(oper) = oper.as_object() else {
        return json_display(oper);
    };
    let name = oper
        .get("name")
        .map(json_display)
        .unwrap_or_else(|| "未知干员".to_string());
    let skill = oper
        .get("skill")
        .map(json_display)
        .map(|skill| format!("技能 {skill}"))
        .unwrap_or_else(|| "技能未指定".to_string());
    let usage = oper
        .get("skill_usage")
        .map(json_display)
        .map(|usage| format!("，模式 {usage}"))
        .unwrap_or_default();
    let requirements = oper
        .get("requirements")
        .and_then(JsonValue::as_object)
        .map(format_operator_requirements)
        .unwrap_or_default();
    format!("{name}（{skill}{usage}{requirements}）")
}

fn format_operator_requirements(requirements: &serde_json::Map<String, JsonValue>) -> String {
    let fields = [
        ("elite", "精英化"),
        ("skill_level", "技能等级"),
        ("module", "模组"),
        ("level", "等级"),
        ("potential", "潜能"),
    ];
    let values = fields
        .iter()
        .filter_map(|(key, label)| {
            requirements
                .get(*key)
                .map(|value| format!("{label} {}", json_display(value)))
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        String::new()
    } else {
        format!(" · {}", values.join(" · "))
    }
}

fn json_display(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Null => "未指定".to_string(),
        _ => value.to_string(),
    }
}

#[derive(Debug)]
enum ParsedSingleSource {
    Remote(u64),
    Local(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedRemoteCode {
    Legacy(u64),
    Single(u64),
    Set(u64),
}

fn parse_remote_code(value: &str) -> Result<Option<ParsedRemoteCode>> {
    if let Some(code) = value.strip_prefix("prts://s") {
        return parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)));
    }
    if let Some(code) = value.strip_prefix("prts://") {
        return if let Some(code) = code.strip_suffix('s') {
            parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)))
        } else {
            parse_remote_id(code, "作业代码").map(|id| Some(ParsedRemoteCode::Single(id)))
        };
    }
    if let Some(code) = value.strip_prefix("maa://") {
        return if let Some(code) = code.strip_suffix('s') {
            parse_remote_id(code, "作业集代码").map(|id| Some(ParsedRemoteCode::Set(id)))
        } else {
            parse_remote_id(code, "作业代码").map(|id| Some(ParsedRemoteCode::Legacy(id)))
        };
    }
    Ok(None)
}

fn parse_remote_id(value: &str, label: &str) -> Result<u64> {
    if value.is_empty() {
        bail!("{label}为空");
    }
    value.parse().with_context(|| format!("{label}无效"))
}

fn parse_single_source(input: &str) -> Result<ParsedSingleSource> {
    let value = input.trim();
    if value.is_empty() {
        bail!("请输入作业代码、maa:// URI 或本地 JSON 路径");
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return Ok(ParsedSingleSource::Remote(value.parse()?));
    }
    match parse_remote_code(value)? {
        Some(ParsedRemoteCode::Legacy(id) | ParsedRemoteCode::Single(id)) => {
            return Ok(ParsedSingleSource::Remote(id));
        }
        Some(ParsedRemoteCode::Set(_)) => {
            bail!("这是作业集代码，请使用“添加作业集”");
        }
        None => {}
    }
    let path = value.strip_prefix("file://").unwrap_or(value);
    if Path::new(path).extension().is_some() || Path::new(path).is_file() {
        return Ok(ParsedSingleSource::Local(PathBuf::from(path)));
    }
    bail!("无法识别作业来源；支持纯数字、maa://、prts:// 和本地 JSON")
}

fn parse_set_code(input: &str) -> Result<u64> {
    let value = input.trim();
    if value.is_empty() {
        bail!("请输入作业集代码");
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value.parse().context("作业集代码无效");
    }
    match parse_remote_code(value)? {
        Some(ParsedRemoteCode::Legacy(id) | ParsedRemoteCode::Set(id)) => Ok(id),
        Some(ParsedRemoteCode::Single(_)) => {
            bail!("这是单作业代码，请使用“添加单作业”")
        }
        None => bail!("无法识别作业集代码；支持纯数字、prts://s<id> 和旧 maa://<id>"),
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    status_code: u16,
    message: Option<String>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct SingleData {
    content: JsonValue,
}

#[derive(Debug, Deserialize)]
struct SetData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    copilot_ids: Vec<u64>,
}

#[derive(Debug)]
struct ParsedCopilot {
    content: JsonValue,
    stage_name: String,
    title: Option<String>,
    difficulty: i64,
}

impl ParsedCopilot {
    fn into_entries(self, source: CopilotEntrySource, origin: CopilotOrigin) -> Vec<CopilotEntry> {
        let mut entries = Vec::new();
        let difficulty = if self.difficulty == 0 {
            1
        } else {
            self.difficulty
        };
        if difficulty & 1 != 0 {
            entries.push(CopilotEntry {
                enabled: true,
                stage_name: self.stage_name.clone(),
                title: self.title.clone(),
                is_raid: false,
                source: source.clone(),
                origin: origin.clone(),
            });
        }
        if difficulty & 2 != 0 {
            entries.push(CopilotEntry {
                enabled: true,
                stage_name: self.stage_name,
                title: self.title,
                is_raid: true,
                source,
                origin,
            });
        }
        entries
    }
}

fn copilot_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(REQUEST_TIMEOUT)
        .timeout_write(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .build()
}

fn fetch_remote_copilot(agent: &ureq::Agent, id: u64, files_dir: &Path) -> Result<ParsedCopilot> {
    let url = format!("{COPILOT_API}{id}");
    let response = agent
        .get(&url)
        .set("User-Agent", "MaaTUI/0.1")
        .call()
        .with_context(|| format!("请求作业失败: {url}"))?;
    let raw = response
        .into_string()
        .with_context(|| format!("读取作业响应失败: {url}"))?;
    let envelope: ApiResponse<SingleData> =
        serde_json::from_str(&raw).with_context(|| format!("解析作业响应失败: {url}"))?;
    if envelope.status_code != 200 {
        bail!(
            "作业站返回 {}: {}",
            envelope.status_code,
            envelope.message.unwrap_or_else(|| "作业不存在".to_string())
        );
    }
    let content = envelope.data.context("作业响应缺少 data")?.content;
    let content = match content {
        JsonValue::String(raw) => serde_json::from_str(&raw).context("解析嵌套作业 JSON 失败")?,
        value => value,
    };
    let parsed = parse_copilot_content(&content)?;
    fs::create_dir_all(files_dir)
        .with_context(|| format!("创建作业缓存目录失败: {}", files_dir.display()))?;
    let path = files_dir.join(format!("{id}.json"));
    let raw = format!("{}\n", serde_json::to_string_pretty(&parsed.content)?);
    atomic_write(&path, raw.as_bytes())?;
    Ok(parsed)
}

fn fetch_copilot_set(agent: &ureq::Agent, id: u64) -> Result<SetData> {
    let url = format!("{COPILOT_SET_API}{id}");
    let response = agent
        .get(&url)
        .set("User-Agent", "MaaTUI/0.1")
        .call()
        .with_context(|| format!("请求作业集失败: {url}"))?;
    let raw = response
        .into_string()
        .with_context(|| format!("读取作业集响应失败: {url}"))?;
    let envelope: ApiResponse<SetData> =
        serde_json::from_str(&raw).with_context(|| format!("解析作业集响应失败: {url}"))?;
    if envelope.status_code != 200 {
        bail!(
            "作业站返回 {}: {}",
            envelope.status_code,
            envelope
                .message
                .unwrap_or_else(|| "作业集不存在".to_string())
        );
    }
    envelope.data.context("作业集响应缺少 data")
}

fn parse_copilot_content(content: &JsonValue) -> Result<ParsedCopilot> {
    let object = content.as_object().context("作业内容不是 JSON 对象")?;
    if object
        .get("type")
        .and_then(JsonValue::as_str)
        .is_some_and(|task_type| task_type.eq_ignore_ascii_case("SSS"))
    {
        bail!("暂不支持 SSS/保全派驻作业");
    }
    let raw_stage_name = object
        .get("stage_name")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|stage| !stage.is_empty())
        .context("作业缺少 stage_name")?;
    let stage_name = tile_alias::resolve_stage_code(raw_stage_name)
        .unwrap_or_else(|| raw_stage_name.to_string());
    let difficulty = object
        .get("difficulty")
        .and_then(JsonValue::as_i64)
        .unwrap_or(0);
    if !(0..=3).contains(&difficulty) {
        bail!("作业 difficulty 必须为 0-3");
    }
    let title = content
        .pointer("/doc/title")
        .or_else(|| content.pointer("/documentation/title"))
        .and_then(JsonValue::as_str)
        .and_then(|title| non_empty(title.to_string()));
    Ok(ParsedCopilot {
        content: content.clone(),
        stage_name,
        title,
        difficulty,
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maatui-copilot-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn single_mode_current_copilot_cannot_switch_difficulty() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let source = dir.join("normal.json");
        fs::write(&source, r#"{"stage_name":"TO-1","difficulty":1}"#).unwrap();
        let mut cache = CopilotCache::load(path, files).unwrap();
        cache
            .replace_current_single(Some(CopilotEntry {
                enabled: true,
                stage_name: "TO-1".to_string(),
                title: None,
                is_raid: false,
                source: CopilotEntrySource::Local { path: source },
                origin: CopilotOrigin::Single,
            }))
            .unwrap();

        assert_eq!(
            cache.current_single_supported_modes().unwrap(),
            (true, false)
        );
        assert!(cache.toggle_current_single_raid().is_err());
        assert!(!cache.current_single().unwrap().is_raid);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copilot_agent_has_bounded_io_timeouts() {
        let debug = format!("{:?}", copilot_agent());
        assert!(debug.contains("timeout_connect: Some(10s)"));
        assert!(debug.contains("timeout_read: Some(30s)"));
        assert!(debug.contains("timeout_write: Some(10s)"));
        assert!(debug.contains("timeout: Some(30s)"));
    }

    #[test]
    fn parses_current_and_legacy_short_codes_by_context() {
        assert_eq!(
            parse_remote_code("prts://s50501").unwrap(),
            Some(ParsedRemoteCode::Set(50501))
        );
        assert_eq!(
            parse_remote_code("prts://98652").unwrap(),
            Some(ParsedRemoteCode::Single(98652))
        );
        assert_eq!(
            parse_remote_code("maa://50501").unwrap(),
            Some(ParsedRemoteCode::Legacy(50501))
        );

        for source in ["123", "prts://98652", "maa://98652"] {
            assert!(matches!(
                parse_single_source(source).unwrap(),
                ParsedSingleSource::Remote(_)
            ));
        }
        assert_eq!(parse_set_code("50501").unwrap(), 50501);
        assert_eq!(parse_set_code("prts://s50501").unwrap(), 50501);
        assert_eq!(parse_set_code("maa://50501").unwrap(), 50501);

        // 保留 maa-cli 与 MaaTUI 之前接受过的尾缀格式。
        assert_eq!(parse_set_code("maa://50501s").unwrap(), 50501);
        assert_eq!(parse_set_code("prts://50501s").unwrap(), 50501);
    }

    #[test]
    fn rejects_short_codes_used_in_the_wrong_context() {
        let single_error = parse_single_source("prts://s50501").unwrap_err();
        assert!(single_error.to_string().contains("添加作业集"));

        let set_error = parse_set_code("prts://98652").unwrap_err();
        assert!(set_error.to_string().contains("添加单作业"));

        for invalid in ["prts://s", "prts://sabc", "maa://s"] {
            assert!(parse_set_code(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    #[ignore = "需要访问真实 PRTS API"]
    fn imports_live_prts_prefixed_set() {
        let dir = temp_dir();
        let files = dir.join("copilot");
        let report = import_source("prts://s50501", ImportKind::Set, &files, |_| {}).unwrap();
        assert_eq!(report.set_id, Some(50501));
        assert_eq!(report.set_name.as_deref(), Some("26夏活"));
        assert_eq!(report.entries.len(), 9);
        assert!(report.entries.iter().all(|entry| {
            matches!(
                &entry.origin,
                CopilotOrigin::Set { id: 50501, name } if name.as_deref() == Some("26夏活")
            )
        }));
        assert!(report.errors.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expands_difficulty_bitmask() {
        for (difficulty, expected) in [
            (0, vec![false]),
            (1, vec![false]),
            (2, vec![true]),
            (3, vec![false, true]),
        ] {
            let parsed = parse_copilot_content(&json!({
                "stage_name": "TO-1",
                "difficulty": difficulty,
                "doc": {"title": "测试作业"}
            }))
            .unwrap();
            let entries =
                parsed.into_entries(CopilotEntrySource::Remote { id: 1 }, CopilotOrigin::Single);
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.is_raid)
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.title.as_deref() == Some("测试作业"))
            );
            assert!(
                entries
                    .iter()
                    .all(|entry| entry.origin == CopilotOrigin::Single)
            );
        }
    }

    #[test]
    fn rejects_sss_and_invalid_content() {
        assert!(parse_copilot_content(&json!({"type": "SSS", "stage_name": "x"})).is_err());
        assert!(parse_copilot_content(&json!({"difficulty": 1})).is_err());
        assert!(parse_copilot_content(&json!({"stage_name": "TO-1", "difficulty": 4})).is_err());
    }

    #[test]
    fn imports_local_copilot_as_single_origin() {
        let dir = temp_dir();
        let path = dir.join("local.json");
        let files = dir.join("files");
        fs::write(
            &path,
            r#"{"stage_name":"TO-1","difficulty":3,"doc":{"title":"本地作业"}}"#,
        )
        .unwrap();

        let report =
            import_source(path.to_str().unwrap(), ImportKind::Single, &files, |_| {}).unwrap();

        assert_eq!(report.set_id, None);
        assert_eq!(report.entries.len(), 2);
        assert!(
            report
                .entries
                .iter()
                .all(|entry| entry.origin == CopilotOrigin::Single)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_persists_run_settings() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let mut cache = CopilotCache::load(path.clone(), files.clone()).unwrap();
        cache
            .set_settings(CopilotOptions {
                formation: true,
                formation_index: 2,
                use_sanity_potion: true,
                add_trust: true,
                ignore_requirements: true,
                support_unit_usage: 3,
                support_unit_name: "能天使".to_string(),
                loop_times: 4,
            })
            .unwrap();

        let reloaded = CopilotCache::load(path, files).unwrap();
        assert_eq!(reloaded.settings().formation_index, 2);
        assert_eq!(reloaded.settings().support_unit_name, "能天使");
        assert_eq!(reloaded.settings().loop_times, 4);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v3_migration_preserves_current_single_and_entries() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        fs::write(
            &path,
            r#"{
                "version": 3,
                "current_single": {
                    "enabled": true,
                    "stage_name": "TO-1",
                    "title": "当前作业",
                    "is_raid": false,
                    "source": {"kind": "remote", "id": 1}
                },
                "entries": [{
                    "enabled": true,
                    "stage_name": "TO-2",
                    "title": "批量作业",
                    "is_raid": false,
                    "source": {"kind": "remote", "id": 2},
                    "origin": {"kind": "set", "id": 50501}
                }]
            }"#,
        )
        .unwrap();

        let cache = CopilotCache::load(path.clone(), files).unwrap();
        assert_eq!(cache.current_single().unwrap().stage_name, "TO-1");
        assert_eq!(cache.entry(0).unwrap().stage_name, "TO-2");
        assert_eq!(cache.settings(), &CopilotOptions::default());
        let persisted: JsonValue =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(persisted["version"], CACHE_VERSION);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_persists_set_origin_metadata() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let mut cache = CopilotCache::load(path.clone(), files.clone()).unwrap();
        cache
            .append(vec![CopilotEntry {
                enabled: true,
                stage_name: "TO-1".to_string(),
                title: Some("集合条目".to_string()),
                is_raid: false,
                source: CopilotEntrySource::Remote { id: 1 },
                origin: CopilotOrigin::Set {
                    id: 50501,
                    name: Some("测试集".to_string()),
                },
            }])
            .unwrap();

        let reloaded = CopilotCache::load(path, files).unwrap();

        assert!(!reloaded.migrated_single_reset_for_test());
        assert_eq!(
            reloaded.entry(0).unwrap().origin,
            CopilotOrigin::Set {
                id: 50501,
                name: Some("测试集".to_string()),
            }
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loads_legacy_cache_as_single_entries() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        fs::write(
            &path,
            r#"{
                "version": 1,
                "entries": [{
                    "enabled": true,
                    "stage_name": "TO-1",
                    "title": "旧作业",
                    "is_raid": false,
                    "source": {"kind": "remote", "id": 1}
                }, {
                    "enabled": false,
                    "stage_name": "TO-2",
                    "title": "旧作业集条目",
                    "is_raid": true,
                    "source": {"kind": "remote", "id": 2},
                    "origin": {"kind": "set", "id": 50501, "name": "旧作业集"}
                }]
            }"#,
        )
        .unwrap();

        let cache = CopilotCache::load(path.clone(), files.clone()).unwrap();
        assert!(cache.migrated_single_reset_for_test());
        assert!(cache.current_single().is_none());
        assert_eq!(cache.entries().len(), 1);
        assert_eq!(cache.entry(0).unwrap().stage_name, "TO-2");
        assert!(cache.entry(0).unwrap().is_raid);
        assert!(!cache.entry(0).unwrap().enabled);
        let persisted: JsonValue =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted["version"], CACHE_VERSION);
        assert_eq!(persisted["entries"].as_array().unwrap().len(), 1);
        let reloaded = CopilotCache::load(path, files).unwrap();
        assert!(!reloaded.migrated_single_reset());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_operations_persist_and_reload() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let mut cache = CopilotCache::load(path.clone(), files.clone()).unwrap();
        cache
            .append(vec![CopilotEntry {
                enabled: true,
                stage_name: "TO-1".to_string(),
                title: None,
                is_raid: false,
                source: CopilotEntrySource::Remote { id: 1 },
                origin: CopilotOrigin::Single,
            }])
            .unwrap();
        assert!(!cache.toggle(0).unwrap());
        let reloaded = CopilotCache::load(path, files).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert!(!reloaded.entry(0).unwrap().enabled);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_rolls_back_when_persistence_fails() {
        let dir = temp_dir();
        let blocker = dir.join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        let mut cache =
            CopilotCache::load(blocker.join("copilot-set.json"), dir.join("files")).unwrap();
        let result = cache.append(vec![CopilotEntry {
            enabled: true,
            stage_name: "TO-1".to_string(),
            title: None,
            is_raid: false,
            source: CopilotEntrySource::Remote { id: 1 },
            origin: CopilotOrigin::Single,
        }]);
        assert!(result.is_err());
        assert!(cache.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enabled_indices_preserve_list_order() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let mut cache = CopilotCache::load(path, files).unwrap();
        cache
            .append(
                (1..=3)
                    .map(|id| CopilotEntry {
                        enabled: true,
                        stage_name: format!("TO-{id}"),
                        title: None,
                        is_raid: false,
                        source: CopilotEntrySource::Remote { id },
                        origin: CopilotOrigin::Single,
                    })
                    .collect(),
            )
            .unwrap();
        cache.toggle(1).unwrap();
        assert_eq!(cache.enabled_indices(), vec![0, 2]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn formats_operator_skill_module_groups_and_notes() {
        let entry = CopilotEntry {
            enabled: true,
            stage_name: "TO-9".to_string(),
            title: Some("详情作业".to_string()),
            is_raid: true,
            source: CopilotEntrySource::Remote { id: 9 },
            origin: CopilotOrigin::Set {
                id: 50501,
                name: Some("测试集".to_string()),
            },
        };
        let detail = format_copilot_detail(
            &entry,
            &json!({
                "version": 3,
                "minimum_required": "v6.0.0",
                "opers": [{
                    "name": "逻各斯",
                    "skill": 3,
                    "requirements": {"elite": 2, "skill_level": 10, "module": 4}
                }],
                "groups": [{
                    "name": "替补组",
                    "opers": [{"name": "阿米娅", "skill": 2, "requirements": {"module": 1}}]
                }],
                "actions": [{
                    "type": "SkillUsage",
                    "name": "逻各斯",
                    "skill_times": 2,
                    "kills": 10,
                    "pre_delay": 1500,
                    "doc": "等待敌人出现"
                }],
                "doc": {"details": "备注一\n备注二"}
            }),
        );
        let text = detail.lines.join("\n");
        assert!(text.contains("关卡名：TO-9"));
        assert!(text.contains("难度：突袭"));
        assert!(text.contains("逻各斯 · 技能 3"));
        assert!(text.contains("模组 4"));
        assert!(text.contains("替补组：阿米娅（技能 2"));
        assert!(text.contains("次数 2"));
        assert!(text.contains("击杀 10"));
        assert!(text.contains("延迟(ms) 1500"));
        assert!(text.contains("等待敌人出现"));
        assert!(text.contains("备注一"));
    }

    #[test]
    fn writes_mixed_raid_batch_task() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        fs::create_dir_all(&files).unwrap();
        fs::write(files.join("1.json"), "{}").unwrap();
        fs::write(files.join("2.json"), "{}").unwrap();
        let mut cache = CopilotCache::load(path, files).unwrap();
        cache
            .append(vec![
                CopilotEntry {
                    enabled: true,
                    stage_name: "TO-1".to_string(),
                    title: None,
                    is_raid: false,
                    source: CopilotEntrySource::Remote { id: 1 },
                    origin: CopilotOrigin::Single,
                },
                CopilotEntry {
                    enabled: true,
                    stage_name: "TO-2".to_string(),
                    title: None,
                    is_raid: true,
                    source: CopilotEntrySource::Remote { id: 2 },
                    origin: CopilotOrigin::Set {
                        id: 50501,
                        name: Some("测试作业集".to_string()),
                    },
                },
            ])
            .unwrap();
        let task = write_batch_task_in(
            &cache,
            vec![0, 1],
            &CopilotRunOptions {
                formation_index: 2,
                use_sanity_potion: true,
                add_trust: true,
                ignore_requirements: false,
                support_unit_usage: 1,
                support_unit_name: String::new(),
            },
            &dir,
        )
        .unwrap();
        let value: JsonValue =
            serde_json::from_str(&fs::read_to_string(&task.path).unwrap()).unwrap();
        let list = value
            .pointer("/tasks/0/params/copilot_list")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(list[0]["is_raid"], false);
        assert_eq!(list[1]["is_raid"], true);
        assert_eq!(cache.enabled_indices(), vec![0, 1]);
        assert_eq!(cache.indices_for_origin(false), vec![0]);
        assert_eq!(cache.indices_for_origin(true), vec![1]);
        assert_eq!(
            value.pointer("/tasks/0/params/formation"),
            Some(&JsonValue::Bool(true))
        );
        assert!(value.pointer("/tasks/0/params/loop_times").is_none());
        remove_batch_task(&task.path);
        fs::remove_dir_all(dir).unwrap();
    }
}
