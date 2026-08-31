//! TOML/JSON/YAML 任务结构校验。

use anyhow::{Context, Result, bail};
use serde_json::Value as JsonValue;
use toml_edit::{Item, Table};

pub(crate) fn validate_toml_task(task: &Table, index: usize) -> Result<()> {
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

pub(crate) fn validate_toml_variant(
    variant: &Table,
    task_index: usize,
    variant_index: usize,
) -> Result<()> {
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

pub(crate) fn validate_json_task(task: &JsonValue, index: usize) -> Result<()> {
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

pub(crate) fn validate_json_variant(
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
