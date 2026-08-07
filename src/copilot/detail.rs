//! 作业详情读取、难度识别与展示格式化。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value as JsonValue;

use super::cache::{CopilotCache, CopilotEntry};

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

pub(super) fn supported_modes_from_path(entry: &CopilotEntry, path: &Path) -> Result<(bool, bool)> {
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

pub(super) fn format_copilot_detail(entry: &CopilotEntry, content: &JsonValue) -> CopilotDetail {
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
