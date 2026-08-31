//! 配置文档及格式元数据。

use serde_json::Value as JsonValue;
use toml_edit::DocumentMut;

use anyhow::Result;

use super::{
    FieldValue, field_from_json, field_from_toml_item, field_to_json, field_to_toml_item,
    json_task, json_task_mut, json_variant, json_variant_mut, toml_task, toml_task_mut,
    toml_variant, toml_variant_mut,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Yaml,
    Json,
}

#[derive(Debug, Clone)]
pub(crate) enum ConfigData {
    Toml(DocumentMut),
    Structured(JsonValue),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum NodePath<'a> {
    TaskField {
        task: usize,
        key: &'a str,
    },
    Param {
        task: usize,
        key: &'a str,
    },
    VariantParam {
        task: usize,
        variant: usize,
        key: &'a str,
    },
    VariantCondition {
        task: usize,
        variant: usize,
        key: &'a str,
    },
}

pub(crate) trait ConfigStore {
    fn task_count(&self) -> usize;
    fn get(&self, path: NodePath<'_>) -> Option<FieldValue>;
    fn set(&mut self, path: NodePath<'_>, value: FieldValue) -> Result<()>;
}

impl ConfigStore for ConfigData {
    fn task_count(&self) -> usize {
        match self {
            Self::Toml(doc) => doc
                .get("tasks")
                .and_then(toml_edit::Item::as_array_of_tables)
                .map_or(0, toml_edit::ArrayOfTables::len),
            Self::Structured(root) => root
                .get("tasks")
                .and_then(JsonValue::as_array)
                .map_or(0, Vec::len),
        }
    }

    fn get(&self, path: NodePath<'_>) -> Option<FieldValue> {
        match (self, path) {
            (Self::Toml(doc), NodePath::TaskField { task, key }) => toml_task(doc, task)?
                .get(key)
                .and_then(field_from_toml_item),
            (Self::Structured(root), NodePath::TaskField { task, key }) => {
                json_task(root, task)?.get(key).and_then(field_from_json)
            }
            (Self::Toml(doc), NodePath::Param { task, key }) => toml_task(doc, task)?
                .get("params")?
                .as_table_like()?
                .get(key)
                .and_then(field_from_toml_item),
            (Self::Structured(root), NodePath::Param { task, key }) => json_task(root, task)?
                .get("params")?
                .get(key)
                .and_then(field_from_json),
            (Self::Toml(doc), NodePath::VariantParam { task, variant, key }) => {
                toml_variant(doc, task, variant)?
                    .get("params")?
                    .as_table_like()?
                    .get(key)
                    .and_then(field_from_toml_item)
            }
            (Self::Structured(root), NodePath::VariantParam { task, variant, key }) => {
                json_variant(root, task, variant)?
                    .get("params")?
                    .get(key)
                    .and_then(field_from_json)
            }
            (Self::Toml(doc), NodePath::VariantCondition { task, variant, key }) => {
                toml_variant(doc, task, variant)?
                    .get("condition")?
                    .as_table_like()?
                    .get(key)
                    .and_then(field_from_toml_item)
            }
            (Self::Structured(root), NodePath::VariantCondition { task, variant, key }) => {
                json_variant(root, task, variant)?
                    .get("condition")?
                    .get(key)
                    .and_then(field_from_json)
            }
        }
    }

    fn set(&mut self, path: NodePath<'_>, value: FieldValue) -> Result<()> {
        match (self, path) {
            (Self::Toml(doc), NodePath::TaskField { task, key }) => {
                toml_task_mut(doc, task)?.insert(key, field_to_toml_item(value));
            }
            (Self::Structured(root), NodePath::TaskField { task, key }) => {
                json_task_mut(root, task)?.insert(key.to_string(), field_to_json(value));
            }
            (Self::Toml(doc), NodePath::Param { task, key }) => {
                let task = toml_task_mut(doc, task)?;
                if !task.contains_key("params") {
                    task.insert("params", toml_edit::value(toml_edit::InlineTable::new()));
                }
                task.get_mut("params")
                    .and_then(toml_edit::Item::as_table_like_mut)
                    .ok_or_else(|| anyhow::anyhow!("params 不是对象"))?
                    .insert(key, field_to_toml_item(value));
            }
            (Self::Structured(root), NodePath::Param { task, key }) => {
                let task = json_task_mut(root, task)?;
                if !task.contains_key("params") {
                    task.insert("params".to_string(), JsonValue::Object(Default::default()));
                }
                task.get_mut("params")
                    .and_then(JsonValue::as_object_mut)
                    .ok_or_else(|| anyhow::anyhow!("params 不是对象"))?
                    .insert(key.to_string(), field_to_json(value));
            }
            (Self::Toml(doc), NodePath::VariantParam { task, variant, key }) => {
                set_toml_variant_field(doc, task, variant, "params", key, value)?;
            }
            (Self::Toml(doc), NodePath::VariantCondition { task, variant, key }) => {
                set_toml_variant_field(doc, task, variant, "condition", key, value)?;
            }
            (Self::Structured(root), NodePath::VariantParam { task, variant, key }) => {
                set_json_variant_field(root, task, variant, "params", key, value)?;
            }
            (Self::Structured(root), NodePath::VariantCondition { task, variant, key }) => {
                set_json_variant_field(root, task, variant, "condition", key, value)?;
            }
        }
        Ok(())
    }
}

fn set_toml_variant_field(
    doc: &mut DocumentMut,
    task: usize,
    variant: usize,
    section: &str,
    key: &str,
    value: FieldValue,
) -> Result<()> {
    let variant = toml_variant_mut(doc, task, variant)?;
    if !variant.contains_key(section) {
        variant.insert(section, toml_edit::value(toml_edit::InlineTable::new()));
    }
    variant
        .get_mut(section)
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| anyhow::anyhow!("{section} 不是对象"))?
        .insert(key, field_to_toml_item(value));
    Ok(())
}

fn set_json_variant_field(
    root: &mut JsonValue,
    task: usize,
    variant: usize,
    section: &str,
    key: &str,
    value: FieldValue,
) -> Result<()> {
    let variant = json_variant_mut(root, task, variant)?;
    if !variant.contains_key(section) {
        variant.insert(section.to_string(), JsonValue::Object(Default::default()));
    }
    variant
        .get_mut(section)
        .and_then(JsonValue::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("{section} 不是对象"))?
        .insert(key.to_string(), field_to_json(value));
    Ok(())
}
