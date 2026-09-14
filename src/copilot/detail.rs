//! 作业详情读取、难度识别与展示格式化。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value as JsonValue;

use super::cache::{CopilotCache, CopilotEntry};

#[derive(Debug, Clone)]
pub struct CopilotDetail {
    pub title: String,
    pub overview: Vec<(String, String)>,
    pub operators: Vec<CopilotOperatorDetail>,
    pub skill_actions: Vec<CopilotSkillActionDetail>,
    pub groups: Vec<CopilotOperatorGroup>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CopilotOperatorDetail {
    pub name: String,
    pub skill: String,
    pub usage: String,
    pub elite: String,
    pub skill_level: String,
    pub module: String,
    pub level: String,
    pub potential: String,
}

#[derive(Debug, Clone)]
pub struct CopilotSkillActionDetail {
    pub name: String,
    pub times: String,
    pub usage: String,
    pub trigger: String,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct CopilotOperatorGroup {
    pub name: String,
    pub operators: Vec<CopilotOperatorDetail>,
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
    let mut overview = vec![
        ("关卡名".to_string(), entry.stage_name.clone()),
        (
            "难度".to_string(),
            if entry.is_raid { "突袭" } else { "普通" }.to_string(),
        ),
        (
            "来源".to_string(),
            format!("{} · {}", entry.origin.label(), entry.source_label()),
        ),
    ];
    if let Some(value) = content.get("minimum_required") {
        overview.push(("最低 Core 版本".to_string(), json_display(value)));
    }
    if let Some(value) = content.get("version") {
        overview.push(("作业版本".to_string(), json_display(value)));
    }

    let operators = content
        .get("opers")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .map(format_operator)
        .collect::<Vec<_>>();

    let skill_actions = content
        .get("actions")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter(|action| {
            matches!(
                action.get("type").and_then(JsonValue::as_str),
                Some("SkillUsage" | "Skill" | "技能用法" | "技能")
            )
        })
        .map(|action| {
            let name = action
                .get("name")
                .map(json_display)
                .unwrap_or_else(|| "未知干员".to_string());
            let times = action
                .get("skill_times")
                .map(json_display)
                .unwrap_or_else(|| "1".to_string());
            let trigger = [
                ("kills", "击杀"),
                ("costs", "费用"),
                ("cost_changes", "费用变化"),
                ("cooling", "冷却干员"),
                ("time_elapsed", "全局计时(ms)"),
                ("pre_delay", "延迟(ms)"),
                ("post_delay", "后延迟(ms)"),
            ]
            .iter()
            .filter_map(|(key, label)| {
                action
                    .get(*key)
                    .map(|value| format!("{label} {}", json_display(value)))
            })
            .collect::<Vec<_>>()
            .join(" · ");
            let note = action
                .get("doc")
                .and_then(JsonValue::as_str)
                .filter(|note| !note.trim().is_empty())
                .unwrap_or("-")
                .to_string();
            let usage = action
                .get("skill_usage")
                .map(json_display)
                .unwrap_or_else(|| "-".to_string());
            CopilotSkillActionDetail {
                name,
                times,
                usage,
                trigger: if trigger.is_empty() {
                    "-".to_string()
                } else {
                    trigger
                },
                note,
            }
        })
        .collect::<Vec<_>>();

    let groups = content
        .get("groups")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .map(|group| {
            let name = group
                .get("name")
                .map(json_display)
                .unwrap_or_else(|| "未命名分组".to_string());
            let operators = group
                .get("opers")
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .map(format_operator)
                .collect();
            CopilotOperatorGroup { name, operators }
        })
        .collect::<Vec<_>>();

    let notes = content
        .pointer("/doc/details")
        .or_else(|| content.pointer("/doc/description"))
        .or_else(|| content.pointer("/documentation/details"))
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let notes = if notes.trim().is_empty() {
        vec!["无".to_string()]
    } else {
        notes.lines().map(str::to_string).collect()
    };

    CopilotDetail {
        title: entry.display_name(),
        overview,
        operators,
        skill_actions,
        groups,
        notes,
    }
}

fn format_operator(oper: &JsonValue) -> CopilotOperatorDetail {
    let name = oper
        .get("name")
        .map(json_display)
        .unwrap_or_else(|| "未知干员".to_string());
    let skill = oper
        .get("skill")
        .map(json_display)
        .unwrap_or_else(|| "0".to_string());
    let usage = oper
        .get("skill_usage")
        .map(json_display)
        .unwrap_or_else(|| "0".to_string());
    let requirements = oper.get("requirements").and_then(JsonValue::as_object);
    let requirement = |key| {
        requirements
            .and_then(|requirements| requirements.get(key))
            .map(json_display)
            .unwrap_or_else(|| "-".to_string())
    };
    let module = requirements
        .and_then(|requirements| requirements.get("module"))
        .filter(|module| module.as_i64() != Some(-1))
        .map(json_display)
        .unwrap_or_else(|| "-".to_string());
    let potential = requirements
        .and_then(|requirements| {
            requirements
                .get("potentiality")
                .or_else(|| requirements.get("potential"))
        })
        .map(json_display)
        .unwrap_or_else(|| "-".to_string());
    CopilotOperatorDetail {
        name,
        skill,
        usage,
        elite: requirement("elite"),
        skill_level: requirement("skill_level"),
        module,
        level: requirement("level"),
        potential,
    }
}

fn json_display(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Null => "未指定".to_string(),
        _ => value.to_string(),
    }
}
