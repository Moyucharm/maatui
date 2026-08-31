//! 为 maa-cli copilot 生成 Tile-Pos 关卡码别名。
//!
//! maa-cli 的 `get_stage_info` 用「文件名以 stage_name 开头」查找地图，
//! 而资源仓库里的文件名多为 `act53side_01-...json`，作业却写 `TO-1`。
//! 在 overview.json 基础上为缺失前缀的 code 建相对符号链接。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;

mod index;
mod paths;

pub(crate) use index::StageAliasIndex;
use paths::tile_pos_dirs;

fn is_safe_stage_code(code: &str) -> bool {
    !code.is_empty()
        && code != "."
        && code != ".."
        && !code.contains('/')
        && !code.contains('\\')
        && !code.contains('\0')
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AliasReport {
    pub created: usize,
    pub skipped: usize,
    pub dirs: usize,
    pub errors: Vec<String>,
}

impl AliasReport {
    pub fn summary(&self) -> Option<String> {
        if self.created == 0 && self.errors.is_empty() {
            return None;
        }
        if self.errors.is_empty() {
            Some(format!(
                "已为 copilot 补齐 {} 个 Tile-Pos 关卡码别名（扫描 {} 个目录）",
                self.created, self.dirs
            ))
        } else {
            Some(format!(
                "Tile-Pos 别名：新建 {}，错误 {}（{}）",
                self.created,
                self.errors.len(),
                self.errors.first().cloned().unwrap_or_default()
            ))
        }
    }
}

/// 将作业 JSON 中的内部地图 ID 解析为 MaaCore 多作业导航使用的关卡码。
#[allow(dead_code)]
pub fn resolve_stage_code(stage_name: &str) -> Option<String> {
    StageAliasIndex::load().resolve(stage_name)
}

/// 在常见资源目录生成关卡码 → 真实 Tile-Pos 文件的符号链接。
pub fn ensure_stage_code_aliases() -> AliasReport {
    let mut report = AliasReport::default();
    for dir in tile_pos_dirs() {
        report.dirs += 1;
        match ensure_aliases_in_dir(&dir) {
            Ok((created, skipped)) => {
                report.created += created;
                report.skipped += skipped;
            }
            Err(error) => report.errors.push(format!("{}: {error}", dir.display())),
        }
    }
    report
}

#[cfg(test)]
fn resolve_stage_code_from_overview(raw: &str, stage_name: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let map = value.as_object()?;
    if let Some(code) = map
        .get(stage_name)
        .and_then(Value::as_object)
        .and_then(|summary| summary.get("code"))
        .and_then(Value::as_str)
        .filter(|code| is_safe_stage_code(code))
    {
        return Some(code.to_string());
    }
    map.values().find_map(|summary| {
        let summary = summary.as_object()?;
        let code = summary
            .get("code")
            .and_then(Value::as_str)
            .filter(|code| is_safe_stage_code(code))?;
        let stage_id = summary.get("stageId").and_then(Value::as_str);
        (code == stage_name || stage_id == Some(stage_name)).then(|| code.to_string())
    })
}

fn ensure_aliases_in_dir(dir: &Path) -> io::Result<(usize, usize)> {
    let overview = dir.join("overview.json");
    let raw = fs::read_to_string(&overview)?;
    ensure_aliases_from_overview(dir, &raw)
}

fn ensure_aliases_from_overview(dir: &Path, raw: &str) -> io::Result<(usize, usize)> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("解析 overview.json 失败: {error}"),
        )
    })?;
    let Some(map) = value.as_object() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "overview.json 不是对象",
        ));
    };

    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_unstable();

    let mut created_codes = HashSet::new();
    let mut created = 0usize;
    let mut skipped = 0usize;

    for summary in map.values() {
        let Some(summary) = summary.as_object() else {
            skipped += 1;
            continue;
        };
        let code = summary.get("code").and_then(Value::as_str).unwrap_or("");
        let filename = summary
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !is_safe_stage_code(code) || filename.is_empty() {
            skipped += 1;
            continue;
        }
        if filename.contains('/') || filename.contains('\\') {
            skipped += 1;
            continue;
        }
        let target = dir.join(filename);
        if !target.exists() {
            skipped += 1;
            continue;
        }
        if created_codes.contains(code) || has_stage_code_file(&names, code) {
            skipped += 1;
            continue;
        }

        let link = dir.join(format!("{code}.json"));
        match std::os::unix::fs::symlink(filename, &link) {
            Ok(()) => {
                created_codes.insert(code.to_string());
                created += 1;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => skipped += 1,
            Err(error) => return Err(error),
        }
    }

    Ok((created, skipped))
}

fn has_stage_code_file(names: &[String], code: &str) -> bool {
    let start = names.partition_point(|name| name.as_str() < code);
    names[start..]
        .iter()
        .take_while(|name| name.starts_with(code))
        .any(|name| filename_matches_stage_code(name, code))
}

fn filename_matches_stage_code(name: &str, code: &str) -> bool {
    let Some(remainder) = name.strip_prefix(code) else {
        return false;
    };
    name.ends_with(".json")
        && (remainder == ".json" || remainder.starts_with('-') || remainder.starts_with('_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maatui-tile-alias-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_internal_stage_id_and_keeps_navigation_code() {
        let overview = r#"{
            "act53side_ex01#f#-activities/act53side/level_act53side_ex01": {
                "code": "TO-EX-1",
                "filename": "act53side_ex01#f#-activities-act53side-level_act53side_ex01.json",
                "stageId": "act53side_ex01#f#"
            },
            "act53side_ex01-activities/act53side/level_act53side_ex01": {
                "code": "TO-EX-1",
                "filename": "act53side_ex01-activities-act53side-level_act53side_ex01.json",
                "stageId": "act53side_ex01"
            }
        }"#;
        assert_eq!(
            resolve_stage_code_from_overview(overview, "act53side_ex01").as_deref(),
            Some("TO-EX-1")
        );
        assert_eq!(
            resolve_stage_code_from_overview(overview, "act53side_ex01#f#").as_deref(),
            Some("TO-EX-1")
        );
        assert_eq!(
            resolve_stage_code_from_overview(overview, "TO-EX-1").as_deref(),
            Some("TO-EX-1")
        );
        assert!(resolve_stage_code_from_overview(overview, "missing").is_none());
    }

    #[test]
    fn creates_code_alias_when_filename_lacks_prefix() {
        let dir = temp_dir();
        fs::write(
            dir.join("act53side_01-level.json"),
            r#"{"code":"TO-1","stageId":"act53side_01"}"#,
        )
        .unwrap();
        let overview = r#"{
            "act53side_01": {
                "code": "TO-1",
                "filename": "act53side_01-level.json"
            }
        }"#;

        let (created, skipped) = ensure_aliases_from_overview(&dir, overview).unwrap();
        assert_eq!(created, 1);
        assert_eq!(skipped, 0);
        let link = dir.join("TO-1.json");
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("act53side_01-level.json")
        );

        let (created_again, _) = ensure_aliases_from_overview(&dir, overview).unwrap();
        assert_eq!(created_again, 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn skips_when_filename_already_starts_with_code() {
        let dir = temp_dir();
        fs::write(dir.join("1-7-something.json"), "{}").unwrap();
        let overview = r#"{
            "main": {"code": "1-7", "filename": "1-7-something.json"}
        }"#;
        let (created, skipped) = ensure_aliases_from_overview(&dir, overview).unwrap();
        assert_eq!(created, 0);
        assert_eq!(skipped, 1);
        assert!(!dir.join("1-7.json").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn longer_stage_code_does_not_shadow_shorter_alias() {
        let dir = temp_dir();
        fs::write(dir.join("TO-10-level.json"), "{}").unwrap();
        fs::write(dir.join("act53side_01-level.json"), "{}").unwrap();
        let overview = r#"{
            "long": {"code": "TO-10", "filename": "TO-10-level.json"},
            "short": {"code": "TO-1", "filename": "act53side_01-level.json"}
        }"#;

        let (created, skipped) = ensure_aliases_from_overview(&dir, overview).unwrap();

        assert_eq!(created, 1);
        assert_eq!(skipped, 1);
        assert!(dir.join("TO-1.json").is_symlink());
        assert!(filename_matches_stage_code("1-1-level.json", "1-1"));
        assert!(!filename_matches_stage_code("1-10-level.json", "1-1"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn indexed_stage_lookup_handles_many_nearby_names() {
        let mut names = (0..5000)
            .map(|index| format!("unrelated-{index:04}.json"))
            .collect::<Vec<_>>();
        names.extend([
            "TO-10-level.json".to_string(),
            "TO-1_level.json".to_string(),
        ]);
        names.sort_unstable();

        assert!(has_stage_code_file(&names, "TO-1"));
        assert!(!has_stage_code_file(&names, "TO-2"));
        assert!(filename_matches_stage_code("TO-1_level.json", "TO-1"));
        assert!(!filename_matches_stage_code("TO-10-level.json", "TO-1"));
    }

    #[test]
    fn rejects_unsafe_codes() {
        assert!(!is_safe_stage_code(""));
        assert!(!is_safe_stage_code(".."));
        assert!(!is_safe_stage_code("a/b"));
        assert!(is_safe_stage_code("TO-1"));
        assert!(is_safe_stage_code("TO-P-1"));
    }
}
