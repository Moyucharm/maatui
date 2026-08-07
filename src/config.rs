//! maa-cli `daily` 配置的定位、无损编辑与原子保存。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map as JsonMap, Value as JsonValue};
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, value};

use crate::storage::{atomic_write, maa_config_dir};

const DAILY_EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Yaml,
    Json,
}

#[derive(Debug, Clone)]
enum ConfigData {
    Toml(DocumentMut),
    Structured(JsonValue),
}

#[derive(Debug)]
pub struct DailyConfig {
    path: PathBuf,
    format: ConfigFormat,
    data: ConfigData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    StringArray(Vec<String>),
    IntegerArray(Vec<i64>),
}

impl FieldValue {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(value) => if *value { "开" } else { "关" }.to_string(),
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::StringArray(values) => values.join(", "),
            Self::IntegerArray(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    pub fn parse_like(input: &str, current: Option<&Self>) -> Result<Self> {
        match current {
            Some(Self::Bool(_)) => match input.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" | "开" => Ok(Self::Bool(true)),
                "0" | "false" | "off" | "no" | "关" => Ok(Self::Bool(false)),
                _ => bail!("请输入 true/false、on/off 或 开/关"),
            },
            Some(Self::Integer(_)) => Ok(Self::Integer(
                input.trim().parse().context("请输入有效整数")?,
            )),
            Some(Self::Float(_)) => {
                Ok(Self::Float(input.trim().parse().context("请输入有效数字")?))
            }
            Some(Self::StringArray(_)) => Ok(Self::StringArray(
                input
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            )),
            Some(Self::IntegerArray(_)) => Ok(Self::IntegerArray(
                input
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| item.parse::<i64>().context("数组中包含无效整数"))
                    .collect::<Result<Vec<_>>>()?,
            )),
            _ => Ok(Self::String(input.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskSummary {
    pub name: String,
    pub task_type: String,
    pub enabled: bool,
}

impl DailyConfig {
    pub fn load_default() -> Result<Self> {
        let config_dir = maa_config_dir()?;
        Self::load_from_tasks_dir(&config_dir.join("tasks"))
    }

    pub fn load_from_tasks_dir(tasks_dir: &Path) -> Result<Self> {
        let matches: Vec<PathBuf> = DAILY_EXTENSIONS
            .iter()
            .map(|ext| tasks_dir.join(format!("daily.{ext}")))
            .filter(|path| path.is_file())
            .collect();

        match matches.as_slice() {
            [] => bail!("未找到 daily.toml、daily.yaml、daily.yml 或 daily.json"),
            [path] => Self::load(path),
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

    pub fn load(path: &Path) -> Result<Self> {
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
            format,
            data,
        };
        config.validate_tasks()?;
        Ok(config)
    }

    pub fn len(&self) -> usize {
        match &self.data {
            ConfigData::Toml(doc) => doc
                .get("tasks")
                .and_then(Item::as_array_of_tables)
                .map_or(0, ArrayOfTables::len),
            ConfigData::Structured(root) => root
                .get("tasks")
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len),
        }
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
        match &self.data {
            ConfigData::Toml(doc) => toml_task(doc, index)?
                .get(key)
                .and_then(field_from_toml_item),
            ConfigData::Structured(root) => {
                json_task(root, index)?.get(key).and_then(field_from_json)
            }
        }
    }

    pub fn set_task_value(&mut self, index: usize, key: &str, field: FieldValue) -> Result<()> {
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    toml_task_mut(doc, index)?.insert(key, field_to_toml_item(field));
                }
                ConfigData::Structured(root) => {
                    json_task_mut(root, index)?.insert(key.to_string(), field_to_json(field));
                }
            }
            Ok(())
        })
    }

    pub fn param_value(&self, index: usize, key: &str) -> Option<FieldValue> {
        match &self.data {
            ConfigData::Toml(doc) => toml_task(doc, index)?
                .get("params")?
                .as_table_like()?
                .get(key)
                .and_then(field_from_toml_item),
            ConfigData::Structured(root) => json_task(root, index)?
                .get("params")?
                .get(key)
                .and_then(field_from_json),
        }
    }

    pub fn set_param_value(&mut self, index: usize, key: &str, field: FieldValue) -> Result<()> {
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let task = toml_task_mut(doc, index)?;
                    if !task.contains_key("params") {
                        task.insert("params", value(InlineTable::new()));
                    }
                    let params = task
                        .get_mut("params")
                        .and_then(Item::as_table_like_mut)
                        .context("params 不是对象")?;
                    params.insert(key, field_to_toml_item(field));
                }
                ConfigData::Structured(root) => {
                    let task = json_task_mut(root, index)?;
                    if !task.contains_key("params") {
                        task.insert("params".to_string(), JsonValue::Object(JsonMap::new()));
                    }
                    task.get_mut("params")
                        .and_then(JsonValue::as_object_mut)
                        .context("params 不是对象")?
                        .insert(key.to_string(), field_to_json(field));
                }
            }
            Ok(())
        })
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
        match &self.data {
            ConfigData::Toml(doc) => toml_variant(doc, task_index, variant_index)?
                .get("params")?
                .as_table_like()?
                .get(key)
                .and_then(field_from_toml_item),
            ConfigData::Structured(root) => json_variant(root, task_index, variant_index)?
                .get("params")?
                .get(key)
                .and_then(field_from_json),
        }
    }

    pub fn set_variant_param_value(
        &mut self,
        task_index: usize,
        variant_index: usize,
        key: &str,
        field: FieldValue,
    ) -> Result<()> {
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let variant = toml_variant_mut(doc, task_index, variant_index)?;
                    if !variant.contains_key("params") {
                        variant.insert("params", value(InlineTable::new()));
                    }
                    variant
                        .get_mut("params")
                        .and_then(Item::as_table_like_mut)
                        .context("params 不是对象")?
                        .insert(key, field_to_toml_item(field));
                }
                ConfigData::Structured(root) => {
                    let variant = json_variant_mut(root, task_index, variant_index)?;
                    if !variant.contains_key("params") {
                        variant.insert("params".to_string(), JsonValue::Object(JsonMap::new()));
                    }
                    variant
                        .get_mut("params")
                        .and_then(JsonValue::as_object_mut)
                        .context("params 不是对象")?
                        .insert(key.to_string(), field_to_json(field));
                }
            }
            Ok(())
        })
    }

    pub fn variant_condition_value(
        &self,
        task_index: usize,
        variant_index: usize,
        key: &str,
    ) -> Option<FieldValue> {
        match &self.data {
            ConfigData::Toml(doc) => toml_variant(doc, task_index, variant_index)?
                .get("condition")?
                .as_table_like()?
                .get(key)
                .and_then(field_from_toml_item),
            ConfigData::Structured(root) => json_variant(root, task_index, variant_index)?
                .get("condition")?
                .get(key)
                .and_then(field_from_json),
        }
    }

    pub fn set_variant_condition_value(
        &mut self,
        task_index: usize,
        variant_index: usize,
        key: &str,
        field: FieldValue,
    ) -> Result<()> {
        self.mutate_and_save(|data| {
            match data {
                ConfigData::Toml(doc) => {
                    let variant = toml_variant_mut(doc, task_index, variant_index)?;
                    if !variant.contains_key("condition") {
                        variant.insert("condition", value(InlineTable::new()));
                    }
                    variant
                        .get_mut("condition")
                        .and_then(Item::as_table_like_mut)
                        .context("condition 不是对象")?
                        .insert(key, field_to_toml_item(field));
                }
                ConfigData::Structured(root) => {
                    let variant = json_variant_mut(root, task_index, variant_index)?;
                    if !variant.contains_key("condition") {
                        variant.insert("condition".to_string(), JsonValue::Object(JsonMap::new()));
                    }
                    variant
                        .get_mut("condition")
                        .and_then(JsonValue::as_object_mut)
                        .context("condition 不是对象")?
                        .insert(key.to_string(), field_to_json(field));
                }
            }
            Ok(())
        })
    }

    pub fn save(&self) -> Result<()> {
        let content = match (&self.format, &self.data) {
            (ConfigFormat::Toml, ConfigData::Toml(doc)) => doc.to_string(),
            (ConfigFormat::Json, ConfigData::Structured(root)) => {
                format!("{}\n", serde_json::to_string_pretty(root)?)
            }
            (ConfigFormat::Yaml, ConfigData::Structured(root)) => serde_yaml::to_string(root)?,
            _ => bail!("配置格式与数据不匹配"),
        };

        atomic_write(&self.path, content.as_bytes())
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
        if let Err(error) = self.save() {
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

fn validate_toml_task(task: &Table, index: usize) -> Result<()> {
    task.get("type")
        .and_then(Item::as_str)
        .with_context(|| format!("任务 {} 缺少字符串 type", index + 1))?;
    if let Some(params) = task.get("params")
        && params.as_table_like().is_none()
    {
        bail!("任务 {} 的 params 不是对象", index + 1);
    }
    if let Some(variants) = task.get("variants") {
        let variants = variants
            .as_array_of_tables()
            .with_context(|| format!("任务 {} 的 variants 不是表数组", index + 1))?;
        for (variant_index, variant) in variants.iter().enumerate() {
            validate_toml_variant(variant, index, variant_index)?;
        }
    }
    Ok(())
}

fn validate_toml_variant(variant: &Table, task_index: usize, variant_index: usize) -> Result<()> {
    for key in ["condition", "params"] {
        if let Some(value) = variant.get(key)
            && value.as_table_like().is_none()
        {
            bail!(
                "任务 {} 的变体 {} 中 {key} 不是对象",
                task_index + 1,
                variant_index + 1
            );
        }
    }
    Ok(())
}

fn validate_json_task(task: &JsonValue, index: usize) -> Result<()> {
    let task = task
        .as_object()
        .with_context(|| format!("任务 {} 不是对象", index + 1))?;
    task.get("type")
        .and_then(JsonValue::as_str)
        .with_context(|| format!("任务 {} 缺少字符串 type", index + 1))?;
    if let Some(params) = task.get("params")
        && !params.is_object()
    {
        bail!("任务 {} 的 params 不是对象", index + 1);
    }
    if let Some(variants) = task.get("variants") {
        let variants = variants
            .as_array()
            .with_context(|| format!("任务 {} 的 variants 不是数组", index + 1))?;
        for (variant_index, variant) in variants.iter().enumerate() {
            validate_json_variant(variant, index, variant_index)?;
        }
    }
    Ok(())
}

fn validate_json_variant(
    variant: &JsonValue,
    task_index: usize,
    variant_index: usize,
) -> Result<()> {
    let variant = variant.as_object().with_context(|| {
        format!(
            "任务 {} 的变体 {} 不是对象",
            task_index + 1,
            variant_index + 1
        )
    })?;
    for key in ["condition", "params"] {
        if let Some(value) = variant.get(key)
            && !value.is_object()
        {
            bail!(
                "任务 {} 的变体 {} 中 {key} 不是对象",
                task_index + 1,
                variant_index + 1
            );
        }
    }
    Ok(())
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
