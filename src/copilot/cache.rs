//! 作业缓存模型、迁移与持久化。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::batch::resolve_entry_path;
use super::detail::supported_modes_from_path;
use crate::storage::{atomic_write, maa_config_dir};

pub(super) const CACHE_VERSION: u32 = 4;

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
