use super::paths::tile_pos_dirs;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;

use super::is_safe_stage_code;

/// 已加载的 Tile-Pos 关卡别名索引。
///
/// 资源目录中的 overview.json 体积较大，导入作业集时不应为每个作业重复读取和解析。
#[derive(Debug, Clone, Default)]
pub(crate) struct StageAliasIndex {
    aliases: HashMap<String, String>,
}

impl StageAliasIndex {
    /// 从当前 Maa 资源目录加载一次所有 overview.json。
    pub(crate) fn load() -> Self {
        let mut index = Self::default();
        for dir in tile_pos_dirs() {
            let overview = dir.join("overview.json");
            let Ok(raw) = fs::read_to_string(overview) else {
                continue;
            };
            index.add_overview(&raw);
        }
        index
    }

    /// 将内部 stage ID 或已有关卡码解析为 MaaCore 使用的关卡码。
    pub(crate) fn resolve(&self, stage_name: &str) -> Option<String> {
        self.aliases
            .get(stage_name)
            .cloned()
            .or_else(|| is_safe_stage_code(stage_name).then(|| stage_name.to_string()))
    }

    fn add_overview(&mut self, raw: &str) {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let Some(map) = value.as_object() else {
            return;
        };
        for (key, summary) in map {
            let Some(summary) = summary.as_object() else {
                continue;
            };
            let Some(code) = summary
                .get("code")
                .and_then(Value::as_str)
                .filter(|code| is_safe_stage_code(code))
            else {
                continue;
            };
            self.aliases.insert(key.clone(), code.to_string());
            if let Some(stage_id) = summary.get("stageId").and_then(Value::as_str) {
                self.aliases.insert(stage_id.to_string(), code.to_string());
            }
            self.aliases.insert(code.to_string(), code.to_string());
        }
    }
}
