//! maa-cli `daily` 配置的定位、格式保留编辑与原子保存。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map as JsonMap, Value as JsonValue};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, value};

use crate::storage::{atomic_write, maa_config_dir};

mod document;
mod model;
mod validation;

use document::{ConfigData, ConfigFormat, ConfigStore, NodePath};
pub use model::{FieldValue, TaskSummary};
use validation::{validate_json_task, validate_toml_task};

const DAILY_EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

#[derive(Debug)]
pub struct DailyConfig {
    path: PathBuf,
    trusted_root: PathBuf,
    format: ConfigFormat,
    data: ConfigData,
    defer_save: bool,
}

impl DailyConfig {
    pub fn load_default() -> Result<Self> {
        let config_dir = maa_config_dir()?;
        Self::load_from_tasks_dir_with_root(&config_dir.join("tasks"), &config_dir)
    }

    #[cfg(test)]
    pub fn load_from_tasks_dir(tasks_dir: &Path) -> Result<Self> {
        Self::load_from_tasks_dir_with_root(tasks_dir, tasks_dir)
    }

    fn load_from_tasks_dir_with_root(tasks_dir: &Path, trusted_root: &Path) -> Result<Self> {
        let matches: Vec<PathBuf> = DAILY_EXTENSIONS
            .iter()
            .map(|ext| tasks_dir.join(format!("daily.{ext}")))
            .filter(|path| path.is_file())
            .collect();

        match matches.as_slice() {
            [] => bail!("未找到 daily.toml、daily.yaml、daily.yml 或 daily.json"),
            [path] => Self::load_with_trusted_root(path, trusted_root),
            _ => bail!(
                "发现多个 daily 配置: {}，请只保留一个",
                matches
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    #[cfg(test)]
    pub fn load(path: &Path) -> Result<Self> {
        let trusted_root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        Self::load_with_trusted_root(path, trusted_root)
    }

    fn load_with_trusted_root(path: &Path, trusted_root: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let (format, data) = match extension.as_str() {
            "toml" => (
                ConfigFormat::Toml,
                ConfigData::Toml(raw.parse::<DocumentMut>().context("解析 TOML 失败")?),
            ),
            "yaml" | "yml" => (
                ConfigFormat::Yaml,
                ConfigData::Structured(serde_yaml::from_str(&raw).context("解析 YAML 失败")?),
            ),
            "json" => (
                ConfigFormat::Json,
                ConfigData::Structured(serde_json::from_str(&raw).context("解析 JSON 失败")?),
            ),
            _ => bail!("不支持的配置格式: {extension}"),
        };

        let config = Self {
            path: path.to_path_buf(),
            trusted_root: trusted_root.to_path_buf(),
            format,
            data,
            defer_save: false,
        };
        config.validate_tasks()?;
        Ok(config)
    }

    pub fn len(&self) -> usize {
        self.data.task_count()
    }

    pub fn task_summaries(&self) -> Vec<TaskSummary> {
        (0..self.len())
            .filter_map(|index| self.task_summary(index))
            .collect()
    }

    pub fn task_summary(&self, index: usize) -> Option<TaskSummary> {
        Some(TaskSummary {
            name: self.task_name(index)?,
            task_type: self.task_type(index)?,
            enabled: self.task_enabled(index).unwrap_or(true),
        })
    }

    /// 将第 `index` 个任务单独导出为一个临时任务文件（强制 `enable = true`），
    /// 返回 `(文件路径, 无扩展名文件名)`。文件名不以 `daily.` 开头，不会干扰日常加载。
    pub fn write_single_task_file(&self, index: usize) -> Result<(PathBuf, String)> {
        let tasks_dir = maa_config_dir()?.join("tasks");
        self.write_single_task_file_into(index, &tasks_dir)
    }

    fn write_single_task_file_into(
        &self,
        index: usize,
        tasks_dir: &Path,
    ) -> Result<(PathBuf, String)> {
        // 带进程号命名，避免多实例并发相互覆盖或残留。
        let basename = format!("maatui-single-daily-{}", std::process::id());
        let (extension, content) = self.single_task_content(index)?;
        fs::create_dir_all(tasks_dir)?;
        let path = tasks_dir.join(format!("{basename}.{extension}"));
        atomic_write(&path, &self.trusted_root, content.as_bytes())?;
        Ok((path, basename))
    }

    /// 将第 `index` 个任务序列化为单独任务文件内容（强制 `enable = true`），
    /// 返回 `(扩展名, 内容)`。
    fn single_task_content(&self, index: usize) -> Result<(String, String)> {
        match (&self.format, &self.data) {
            (ConfigFormat::Toml, ConfigData::Toml(doc)) => {
                let task = toml_task(doc, index).context("任务索引超出范围")?;
                let mut task = task.clone();
                enable_toml_task(&mut task);
                let mut tasks = ArrayOfTables::new();
                tasks.push(task);
                let mut out = DocumentMut::new();
                out.insert("tasks", Item::ArrayOfTables(tasks));
                Ok(("toml".to_string(), out.to_string()))
            }
            (ConfigFormat::Json, ConfigData::Structured(root)) => Ok((
                "json".to_string(),
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&single_task_value(root, index)?)?
                ),
            )),
            (ConfigFormat::Yaml, ConfigData::Structured(root)) => Ok((
                "yaml".to_string(),
                serde_yaml::to_string(&single_task_value(root, index)?)?,
            )),
            _ => bail!("配置格式与数据不匹配"),
        }
    }

    pub fn task_name(&self, index: usize) -> Option<String> {
        self.task_value(index, "name")
            .and_then(|value| match value {
                FieldValue::String(value) => Some(value),
                _ => None,
            })
            .or_else(|| self.task_type(index))
    }

    pub fn task_type(&self, index: usize) -> Option<String> {
        self.task_value(index, "type")
            .and_then(|value| match value {
                FieldValue::String(value) => Some(value),
                _ => None,
            })
    }

    pub fn task_enabled(&self, index: usize) -> Option<bool> {
        match self.param_value(index, "enable") {
            Some(FieldValue::Bool(value)) => Some(value),
            None => Some(true),
            _ => None,
        }
    }

    pub fn toggle_task(&mut self, index: usize) -> Result<bool> {
        let enabled = !self.task_enabled(index).unwrap_or(true);
        self.set_param_value(index, "enable", FieldValue::Bool(enabled))?;
        Ok(enabled)
    }

    pub fn move_task(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.len() || to >= self.len() || from == to {
            return Ok(());
        }
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let tasks = toml_tasks_mut(doc)?;
                    let mut reordered: Vec<Table> = tasks.iter().cloned().collect();
                    let table = reordered.remove(from);
                    reordered.insert(to, table);
                    let mut replacement = ArrayOfTables::new();
                    for table in reordered {
                        replacement.push(table);
                    }
                    *tasks = replacement;
                }
                ConfigData::Structured(root) => {
                    let tasks = json_tasks_mut(root)?;
                    let task = tasks.remove(from);
                    tasks.insert(to, task);
                }
            }
            Ok(())
        })
    }

    pub fn delete_task(&mut self, index: usize) -> Result<()> {
        if index >= self.len() {
            bail!("任务索引超出范围");
        }
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    toml_tasks_mut(doc)?.remove(index);
                }
                ConfigData::Structured(root) => {
                    json_tasks_mut(root)?.remove(index);
                }
            }
            Ok(())
        })
    }

    pub fn add_task(&mut self, task_type: &str) -> Result<usize> {
        let name = default_task_name(task_type);
        self.mutate_and_save(|data| {
            let index = match data {
                ConfigData::Toml(doc) => {
                    let mut task = Table::new();
                    task.insert("name", value(name));
                    task.insert("type", value(task_type));
                    let mut params = InlineTable::new();
                    params.insert("enable", true.into());
                    task.insert("params", value(params));
                    let tasks = toml_tasks_mut(doc)?;
                    tasks.push(task);
                    tasks.len() - 1
                }
                ConfigData::Structured(root) => {
                    let mut params = JsonMap::new();
                    params.insert("enable".to_string(), JsonValue::Bool(true));
                    let mut task = JsonMap::new();
                    task.insert("name".to_string(), JsonValue::String(name.to_string()));
                    task.insert("type".to_string(), JsonValue::String(task_type.to_string()));
                    task.insert("params".to_string(), JsonValue::Object(params));
                    let tasks = json_tasks_mut(root)?;
                    tasks.push(JsonValue::Object(task));
                    tasks.len() - 1
                }
            };
            Ok(index)
        })
    }

    pub fn task_value(&self, index: usize, key: &str) -> Option<FieldValue> {
        self.data.get(NodePath::TaskField { task: index, key })
    }

    pub fn set_task_value(&mut self, index: usize, key: &str, field: FieldValue) -> Result<()> {
        self.mutate_and_save(|data| data.set(NodePath::TaskField { task: index, key }, field))
    }

    pub fn param_value(&self, index: usize, key: &str) -> Option<FieldValue> {
        self.data.get(NodePath::Param { task: index, key })
    }

    pub fn set_param_value(&mut self, index: usize, key: &str, field: FieldValue) -> Result<()> {
        self.mutate_and_save(|data| data.set(NodePath::Param { task: index, key }, field))
    }

    pub fn variant_count(&self, task_index: usize) -> usize {
        match &self.data {
            ConfigData::Toml(doc) => toml_task(doc, task_index)
                .and_then(|task| task.get("variants"))
                .and_then(Item::as_array_of_tables)
                .map_or(0, ArrayOfTables::len),
            ConfigData::Structured(root) => json_task(root, task_index)
                .and_then(|task| task.get("variants"))
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len),
        }
    }

    pub fn variant_summary(&self, task_index: usize, variant_index: usize) -> Option<String> {
        let condition = self
            .variant_condition_value(task_index, variant_index, "type")
            .map_or_else(|| "Always".to_string(), |value| value.display());
        let stage = self
            .variant_param_value(task_index, variant_index, "stage")
            .map(|value| format!(" · stage={}", value.display()))
            .unwrap_or_default();
        Some(format!("{condition}{stage}"))
    }

    pub fn add_variant(&mut self, task_index: usize) -> Result<usize> {
        self.mutate_and_save(|data| {
            let index = match data {
                ConfigData::Toml(doc) => {
                    let task = toml_task_mut(doc, task_index)?;
                    if !task.contains_key("variants") {
                        task.insert("variants", Item::ArrayOfTables(ArrayOfTables::new()));
                    }
                    let variants = task
                        .get_mut("variants")
                        .and_then(Item::as_array_of_tables_mut)
                        .context("variants 不是表数组")?;
                    let mut variant = Table::new();
                    let mut params = InlineTable::new();
                    params.insert("stage", "".into());
                    variant.insert("params", value(params));
                    variants.push(variant);
                    variants.len() - 1
                }
                ConfigData::Structured(root) => {
                    let task = json_task_mut(root, task_index)?;
                    if !task.contains_key("variants") {
                        task.insert("variants".to_string(), JsonValue::Array(Vec::new()));
                    }
                    let variants = task
                        .get_mut("variants")
                        .and_then(JsonValue::as_array_mut)
                        .context("variants 不是数组")?;
                    variants.push(serde_json::json!({"params": {"stage": ""}}));
                    variants.len() - 1
                }
            };
            Ok(index)
        })
    }

    pub fn add_on_side_story_variant(
        &mut self,
        task_index: usize,
        client: &str,
        stage: &str,
    ) -> Result<usize> {
        if client.trim().is_empty() {
            bail!("活动变体需要指定客户端");
        }
        if stage.trim().is_empty() {
            bail!("活动变体需要指定关卡");
        }
        self.mutate_and_save(|data| {
            let index = match data {
                ConfigData::Toml(doc) => {
                    let task = toml_task_mut(doc, task_index)?;
                    if !task.contains_key("variants") {
                        task.insert("variants", Item::ArrayOfTables(ArrayOfTables::new()));
                    }
                    let variants = task
                        .get_mut("variants")
                        .and_then(Item::as_array_of_tables_mut)
                        .context("variants 不是表数组")?;
                    let mut condition = InlineTable::new();
                    condition.insert("type", "OnSideStory".into());
                    condition.insert("client", client.into());
                    let mut params = InlineTable::new();
                    params.insert("stage", stage.into());
                    let mut variant = Table::new();
                    variant.insert("condition", value(condition));
                    variant.insert("params", value(params));
                    variants.push(variant);
                    variants.len() - 1
                }
                ConfigData::Structured(root) => {
                    let task = json_task_mut(root, task_index)?;
                    if !task.contains_key("variants") {
                        task.insert("variants".to_string(), JsonValue::Array(Vec::new()));
                    }
                    let variants = task
                        .get_mut("variants")
                        .and_then(JsonValue::as_array_mut)
                        .context("variants 不是数组")?;
                    variants.push(serde_json::json!({
                        "condition": {"type": "OnSideStory", "client": client},
                        "params": {"stage": stage}
                    }));
                    variants.len() - 1
                }
            };
            Ok(index)
        })
    }

    pub fn delete_variant(&mut self, task_index: usize, variant_index: usize) -> Result<()> {
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let variants = toml_task_mut(doc, task_index)?
                        .get_mut("variants")
                        .and_then(Item::as_array_of_tables_mut)
                        .context("variants 不存在")?;
                    if variant_index >= variants.len() {
                        bail!("变体索引超出范围");
                    }
                    variants.remove(variant_index);
                }
                ConfigData::Structured(root) => {
                    let variants = json_task_mut(root, task_index)?
                        .get_mut("variants")
                        .and_then(JsonValue::as_array_mut)
                        .context("variants 不存在")?;
                    if variant_index >= variants.len() {
                        bail!("变体索引超出范围");
                    }
                    variants.remove(variant_index);
                }
            }
            Ok(())
        })
    }

    pub fn move_variant(&mut self, task_index: usize, from: usize, to: usize) -> Result<()> {
        let len = self.variant_count(task_index);
        if from >= len || to >= len || from == to {
            return Ok(());
        }
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let variants = toml_task_mut(doc, task_index)?
                        .get_mut("variants")
                        .and_then(Item::as_array_of_tables_mut)
                        .context("variants 不存在")?;
                    let mut reordered: Vec<Table> = variants.iter().cloned().collect();
                    let variant = reordered.remove(from);
                    reordered.insert(to, variant);
                    let mut replacement = ArrayOfTables::new();
                    for variant in reordered {
                        replacement.push(variant);
                    }
                    *variants = replacement;
                }
                ConfigData::Structured(root) => {
                    let variants = json_task_mut(root, task_index)?
                        .get_mut("variants")
                        .and_then(JsonValue::as_array_mut)
                        .context("variants 不存在")?;
                    let variant = variants.remove(from);
                    variants.insert(to, variant);
                }
            }
            Ok(())
        })
    }

    pub fn variant_param_value(
        &self,
        task_index: usize,
        variant_index: usize,
        key: &str,
    ) -> Option<FieldValue> {
        self.data.get(NodePath::VariantParam {
            task: task_index,
            variant: variant_index,
            key,
        })
    }

    pub fn set_variant_param_value(
        &mut self,
        task_index: usize,
        variant_index: usize,
        key: &str,
        field: FieldValue,
    ) -> Result<()> {
        self.mutate_and_save(|data| {
            data.set(
                NodePath::VariantParam {
                    task: task_index,
                    variant: variant_index,
                    key,
                },
                field,
            )
        })
    }

    pub fn variant_condition_value(
        &self,
        task_index: usize,
        variant_index: usize,
        key: &str,
    ) -> Option<FieldValue> {
        self.data.get(NodePath::VariantCondition {
            task: task_index,
            variant: variant_index,
            key,
        })
    }

    pub fn set_variant_condition_value(
        &mut self,
        task_index: usize,
        variant_index: usize,
        key: &str,
        field: FieldValue,
    ) -> Result<()> {
        self.mutate_and_save(|data| {
            data.set(
                NodePath::VariantCondition {
                    task: task_index,
                    variant: variant_index,
                    key,
                },
                field,
            )
        })
    }

    pub fn save(&self) -> Result<()> {
        let content = self.serialize_snapshot()?;
        atomic_write(&self.path, &self.trusted_root, content.as_bytes())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn trusted_root(&self) -> &Path {
        &self.trusted_root
    }

    pub(crate) fn set_defer_save(&mut self, defer: bool) {
        self.defer_save = defer;
    }

    pub(crate) fn serialize_snapshot(&self) -> Result<String> {
        Ok(match (&self.format, &self.data) {
            (ConfigFormat::Toml, ConfigData::Toml(doc)) => doc.to_string(),
            (ConfigFormat::Json, ConfigData::Structured(root)) => {
                format!("{}\n", serde_json::to_string_pretty(root)?)
            }
            (ConfigFormat::Yaml, ConfigData::Structured(root)) => serde_yaml::to_string(root)?,
            _ => bail!("配置格式与数据不匹配"),
        })
    }

    fn mutate_and_save<T>(
        &mut self,
        operation: impl FnOnce(&mut ConfigData) -> Result<T>,
    ) -> Result<T> {
        let original = self.data.clone();
        let value = match operation(&mut self.data) {
            Ok(value) => value,
            Err(error) => {
                self.data = original;
                return Err(error);
            }
        };
        if !self.defer_save
            && let Err(error) = self.save()
        {
            self.data = original;
            return Err(error);
        }
        Ok(value)
    }

    fn validate_tasks(&self) -> Result<()> {
        match &self.data {
            ConfigData::Toml(doc) => {
                let tasks = doc
                    .get("tasks")
                    .and_then(Item::as_array_of_tables)
                    .context("配置中缺少 [[tasks]] 数组")?;
                for (index, task) in tasks.iter().enumerate() {
                    validate_toml_task(task, index)?;
                }
            }
            ConfigData::Structured(root) => {
                let tasks = root
                    .get("tasks")
                    .and_then(JsonValue::as_array)
                    .context("配置中缺少 tasks 数组")?;
                for (index, task) in tasks.iter().enumerate() {
                    validate_json_task(task, index)?;
                }
            }
        }
        Ok(())
    }
}

fn default_task_name(task_type: &str) -> &'static str {
    match task_type {
        "StartUp" => "启动游戏",
        "Recruit" => "公开招募",
        "Fight" => "理智作战",
        "Infrast" => "基建换班",
        "Mall" => "信用商店",
        "Award" => "领取奖励",
        "CloseDown" => "关闭游戏",
        "Copilot" => "自动战斗",
        _ => "新任务",
    }
}

/// 强制任务参数中 `enable = true`，保证单独执行时即使原配置关闭也会运行。
fn enable_toml_task(task: &mut Table) {
    if let Some(params) = task.get_mut("params").and_then(Item::as_table_like_mut) {
        params.insert("enable", value(true));
    } else {
        let mut params = InlineTable::new();
        params.insert("enable", true.into());
        task.insert("params", value(params));
    }
}

/// 提取单个任务并强制 `enable = true`，包装为 `{ "tasks": [task] }`。
fn single_task_value(root: &JsonValue, index: usize) -> Result<JsonValue> {
    let task = json_task(root, index).context("任务索引超出范围")?;
    let mut task = task.clone();
    let params = task
        .entry("params")
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if let JsonValue::Object(params) = params {
        params.insert("enable".to_string(), JsonValue::Bool(true));
    }
    let mut wrapper = JsonMap::new();
    wrapper.insert(
        "tasks".to_string(),
        JsonValue::Array(vec![JsonValue::Object(task)]),
    );
    Ok(JsonValue::Object(wrapper))
}

fn toml_tasks_mut(doc: &mut DocumentMut) -> Result<&mut ArrayOfTables> {
    doc.get_mut("tasks")
        .and_then(Item::as_array_of_tables_mut)
        .context("配置中缺少 [[tasks]] 数组")
}

fn toml_task(doc: &DocumentMut, index: usize) -> Option<&Table> {
    doc.get("tasks")?.as_array_of_tables()?.get(index)
}

fn toml_task_mut(doc: &mut DocumentMut, index: usize) -> Result<&mut Table> {
    toml_tasks_mut(doc)?
        .get_mut(index)
        .context("任务索引超出范围")
}

fn toml_variant(doc: &DocumentMut, task_index: usize, variant_index: usize) -> Option<&Table> {
    toml_task(doc, task_index)?
        .get("variants")?
        .as_array_of_tables()?
        .get(variant_index)
}

fn toml_variant_mut(
    doc: &mut DocumentMut,
    task_index: usize,
    variant_index: usize,
) -> Result<&mut Table> {
    toml_task_mut(doc, task_index)?
        .get_mut("variants")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|variants| variants.get_mut(variant_index))
        .context("变体索引超出范围")
}

fn json_tasks_mut(root: &mut JsonValue) -> Result<&mut Vec<JsonValue>> {
    root.get_mut("tasks")
        .and_then(JsonValue::as_array_mut)
        .context("配置中缺少 tasks 数组")
}

fn json_task(root: &JsonValue, index: usize) -> Option<&JsonMap<String, JsonValue>> {
    root.get("tasks")?.as_array()?.get(index)?.as_object()
}

fn json_task_mut(root: &mut JsonValue, index: usize) -> Result<&mut JsonMap<String, JsonValue>> {
    json_tasks_mut(root)?
        .get_mut(index)
        .and_then(JsonValue::as_object_mut)
        .context("任务索引超出范围或任务不是对象")
}

fn json_variant(
    root: &JsonValue,
    task_index: usize,
    variant_index: usize,
) -> Option<&JsonMap<String, JsonValue>> {
    json_task(root, task_index)?
        .get("variants")?
        .as_array()?
        .get(variant_index)?
        .as_object()
}

fn json_variant_mut(
    root: &mut JsonValue,
    task_index: usize,
    variant_index: usize,
) -> Result<&mut JsonMap<String, JsonValue>> {
    json_task_mut(root, task_index)?
        .get_mut("variants")
        .and_then(JsonValue::as_array_mut)
        .and_then(|variants| variants.get_mut(variant_index))
        .and_then(JsonValue::as_object_mut)
        .context("变体索引超出范围或变体不是对象")
}

fn field_from_toml_item(item: &Item) -> Option<FieldValue> {
    let value = item.as_value()?;
    if let Some(value) = value.as_bool() {
        return Some(FieldValue::Bool(value));
    }
    if let Some(value) = value.as_integer() {
        return Some(FieldValue::Integer(value));
    }
    if let Some(value) = value.as_float() {
        return Some(FieldValue::Float(value));
    }
    if let Some(value) = value.as_str() {
        return Some(FieldValue::String(value.to_string()));
    }
    if let Some(array) = value.as_array() {
        if array.iter().all(|value| value.as_integer().is_some()) {
            return Some(FieldValue::IntegerArray(
                array
                    .iter()
                    .filter_map(|value| value.as_integer())
                    .collect(),
            ));
        }
        return Some(FieldValue::StringArray(
            array
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect(),
        ));
    }
    None
}

fn field_to_toml_item(field: FieldValue) -> Item {
    match field {
        FieldValue::Bool(value_) => value(value_),
        FieldValue::Integer(value_) => value(value_),
        FieldValue::Float(value_) => value(value_),
        FieldValue::String(value_) => value(value_),
        FieldValue::StringArray(values) => {
            let mut array = Array::new();
            for value_ in values {
                array.push(value_);
            }
            value(array)
        }
        FieldValue::IntegerArray(values) => {
            let mut array = Array::new();
            for value_ in values {
                array.push(value_);
            }
            value(array)
        }
    }
}

fn field_from_json(value: &JsonValue) -> Option<FieldValue> {
    match value {
        JsonValue::Bool(value) => Some(FieldValue::Bool(*value)),
        JsonValue::Number(value) => value
            .as_i64()
            .map(FieldValue::Integer)
            .or_else(|| value.as_f64().map(FieldValue::Float)),
        JsonValue::String(value) => Some(FieldValue::String(value.clone())),
        JsonValue::Array(values) if values.iter().all(JsonValue::is_i64) => Some(
            FieldValue::IntegerArray(values.iter().filter_map(JsonValue::as_i64).collect()),
        ),
        JsonValue::Array(values) => Some(FieldValue::StringArray(
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect(),
        )),
        _ => None,
    }
}

fn field_to_json(field: FieldValue) -> JsonValue {
    match field {
        FieldValue::Bool(value) => JsonValue::Bool(value),
        FieldValue::Integer(value) => value.into(),
        FieldValue::Float(value) => {
            serde_json::Number::from_f64(value).map_or(JsonValue::Null, JsonValue::Number)
        }
        FieldValue::String(value) => JsonValue::String(value),
        FieldValue::StringArray(values) => {
            JsonValue::Array(values.into_iter().map(JsonValue::String).collect())
        }
        FieldValue::IntegerArray(values) => {
            JsonValue::Array(values.into_iter().map(Into::into).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maatui-config-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn serialize_snapshot_reports_format_mismatch() {
        let config = DailyConfig {
            path: PathBuf::from("daily.toml"),
            trusted_root: PathBuf::from("."),
            format: ConfigFormat::Toml,
            data: ConfigData::Structured(JsonValue::Null),
            defer_save: true,
        };

        assert_eq!(
            config.serialize_snapshot().unwrap_err().to_string(),
            "配置格式与数据不匹配"
        );
    }
    #[test]
    fn edits_toml_without_losing_unknown_fields() {
        let dir = temp_dir();
        let path = dir.join("daily.toml");
        fs::write(
            &path,
            r#"# keep-comment
custom_root = "keep"

[[tasks]]
name = "刷理智"
type = "Fight"
params = { stage = "1-7", custom_param = "keep" }
"#,
        )
        .unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        assert_eq!(config.len(), 1);
        config.toggle_task(0).unwrap();
        config
            .set_param_value(0, "stage", FieldValue::String("CE-6".to_string()))
            .unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# keep-comment"));
        assert!(saved.contains("custom_root = \"keep\""));
        assert!(saved.contains("custom_param = \"keep\""));
        assert!(saved.contains("stage = \"CE-6\""));
        assert!(saved.contains("enable = false"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn single_task_file_contains_only_selected_task_and_forces_enable() {
        let dir = temp_dir();
        let tasks_dir = dir.join("tasks");
        let path = dir.join("daily.toml");
        fs::write(
            &path,
            r#"[[tasks]]
name = "刷理智"
type = "Fight"
params = { enable = false, stage = "1-7" }

[[tasks]]
name = "公招"
type = "Recruit"
params = { enable = true, times = 4 }
"#,
        )
        .unwrap();

        let config = DailyConfig::load(&path).unwrap();
        let (file, base) = config.write_single_task_file_into(1, &tasks_dir).unwrap();
        assert!(base.starts_with("maatui-single-daily-"));

        let content = fs::read_to_string(&file).unwrap();
        assert!(content.contains("公招"));
        assert!(!content.contains("刷理智"));
        assert!(content.contains("enable = true"));

        // 生成的临时文件可被重新加载，且只包含一个已启用任务。
        let reloaded = DailyConfig::load(&file).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.task_enabled(0), Some(true));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reorders_and_deletes_json_tasks() {
        let dir = temp_dir();
        let path = dir.join("daily.json");
        fs::write(
            &path,
            r#"{"unknown":42,"tasks":[{"type":"StartUp"},{"type":"Award"}]}"#,
        )
        .unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        config.move_task(1, 0).unwrap();
        assert_eq!(config.task_type(0).as_deref(), Some("Award"));
        config.delete_task(1).unwrap();
        let saved: JsonValue = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["unknown"], 42);
        assert_eq!(saved["tasks"].as_array().unwrap().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_multiple_daily_files() {
        let dir = temp_dir();
        fs::write(dir.join("daily.toml"), "tasks = []").unwrap();
        fs::write(dir.join("daily.json"), "{\"tasks\":[]}").unwrap();
        let err = DailyConfig::load_from_tasks_dir(&dir).unwrap_err();
        assert!(err.to_string().contains("多个 daily 配置"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rolls_back_memory_when_save_fails() {
        let dir = temp_dir();
        let path = dir.join("daily.json");
        fs::write(
            &path,
            r#"{"tasks":[{"type":"Fight","params":{"enable":true}}]}"#,
        )
        .unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&dir).unwrap();

        assert!(config.toggle_task(0).is_err());
        assert_eq!(config.task_enabled(0), Some(true));
    }

    #[test]
    fn preserves_permissions_when_saving() {
        let dir = temp_dir();
        let path = dir.join("daily.toml");
        fs::write(
            &path,
            "[[tasks]]\ntype = \"Fight\"\nparams = { enable = true }\n",
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        config.toggle_task(0).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_symlink_when_saving() {
        let dir = temp_dir();
        let target = dir.join("actual.toml");
        let path = dir.join("daily.toml");
        fs::write(
            &target,
            "[[tasks]]\ntype = \"Fight\"\nparams = { enable = true }\n",
        )
        .unwrap();
        symlink(&target, &path).unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        config.toggle_task(0).unwrap();

        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("enable = false")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_empty_stage_across_supported_formats() {
        let dir = temp_dir();
        let cases = [
            (
                "daily.toml",
                "[[tasks]]\ntype = \"Fight\"\nparams = { stage = \"1-7\" }\n",
            ),
            (
                "daily.json",
                r#"{"tasks":[{"type":"Fight","params":{"stage":"1-7"}}]}"#,
            ),
            (
                "daily.yaml",
                "tasks:\n  - type: Fight\n    params:\n      stage: 1-7\n",
            ),
        ];

        for (filename, content) in cases {
            let path = dir.join(filename);
            fs::write(&path, content).unwrap();
            let mut config = DailyConfig::load(&path).unwrap();
            config
                .set_param_value(0, "stage", FieldValue::String(String::new()))
                .unwrap();
            let variant = config.add_variant(0).unwrap();
            assert_eq!(
                config.param_value(0, "stage"),
                Some(FieldValue::String(String::new()))
            );
            assert_eq!(
                config.variant_param_value(0, variant, "stage"),
                Some(FieldValue::String(String::new()))
            );

            let reloaded = DailyConfig::load(&path).unwrap();
            assert_eq!(
                reloaded.param_value(0, "stage"),
                Some(FieldValue::String(String::new()))
            );
            assert_eq!(
                reloaded.variant_param_value(0, variant, "stage"),
                Some(FieldValue::String(String::new()))
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_existing_compound_conditions_when_editing_variants() {
        let dir = temp_dir();
        let path = dir.join("daily.toml");
        fs::write(
            &path,
            r#"[[tasks]]
type = "Fight"

[[tasks.variants]]
condition = { type = "And", conditions = [{ type = "Time", start = "08:00:00" }, { type = "Weekday", weekdays = ["Mon"] }] }
params = { stage = "1-7" }
"#,
        )
        .unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        config
            .set_variant_param_value(0, 0, "stage", FieldValue::String("CE-6".to_string()))
            .unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("type = \"And\""));
        assert!(saved.contains("conditions = ["));
        assert!(saved.contains("type = \"Time\""));
        assert!(saved.contains("type = \"Weekday\""));
        assert!(saved.contains("stage = \"CE-6\""));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn adds_on_side_story_variant_without_dropping_existing_data() {
        let dir = temp_dir();
        let path = dir.join("daily.toml");
        fs::write(
            &path,
            "root = \"keep\"\n\n[[tasks]]\ntype = \"Fight\"\nparams = { stage = \"1-7\" }\n",
        )
        .unwrap();

        let mut config = DailyConfig::load(&path).unwrap();
        assert_eq!(
            config
                .add_on_side_story_variant(0, "Official", "CF-8")
                .unwrap(),
            0
        );
        assert_eq!(config.variant_count(0), 1);
        assert_eq!(
            config.variant_condition_value(0, 0, "type"),
            Some(FieldValue::String("OnSideStory".to_string()))
        );
        assert_eq!(
            config.variant_param_value(0, 0, "stage"),
            Some(FieldValue::String("CF-8".to_string()))
        );
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("root = \"keep\"")
        );
        assert!(config.add_on_side_story_variant(0, "Official", "").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_malformed_tasks_and_variants() {
        let dir = temp_dir();
        let bad_task = dir.join("bad-task.json");
        fs::write(&bad_task, r#"{"tasks":[null]}"#).unwrap();
        assert!(
            DailyConfig::load(&bad_task)
                .unwrap_err()
                .to_string()
                .contains("任务 1 不是对象")
        );

        let bad_variant = dir.join("bad-variant.yaml");
        fs::write(
            &bad_variant,
            "tasks:\n  - type: Fight\n    variants:\n      - invalid\n",
        )
        .unwrap();
        assert!(
            DailyConfig::load(&bad_variant)
                .unwrap_err()
                .to_string()
                .contains("变体 1 不是对象")
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
