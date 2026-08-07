//! 自动战斗作业：缓存、导入、详情与批量任务的兼容门面。

mod batch;
mod cache;
mod detail;
mod import;

pub use batch::{BatchTask, CopilotRunOptions, remove_batch_task, write_batch_task};
#[cfg(test)]
pub use cache::CopilotEntrySource;
pub use cache::{CopilotCache, CopilotEntry, CopilotOptions, CopilotOrigin};
pub use detail::CopilotDetail;
pub use import::{ImportKind, ImportProgress, ImportReport, import_source};

#[cfg(test)]
pub(crate) use batch::write_batch_task_in;
#[cfg(test)]
use cache::CACHE_VERSION;
#[cfg(test)]
use detail::format_copilot_detail;
#[cfg(test)]
use import::{
    ParsedRemoteCode, ParsedSingleSource, copilot_agent, parse_copilot_content, parse_remote_code,
    parse_set_code, parse_single_source,
};
#[cfg(test)]
use serde_json::{Value as JsonValue, json};
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::PathBuf;

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
    fn replacing_current_single_does_not_create_history_entries() {
        let dir = temp_dir();
        let path = dir.join("copilot-set.json");
        let files = dir.join("files");
        let first_path = dir.join("first.json");
        let second_path = dir.join("second.json");
        let batch_path = dir.join("batch.json");
        for file in [&first_path, &second_path, &batch_path] {
            fs::write(file, "{}").unwrap();
        }
        let mut cache = CopilotCache::load(path, files).unwrap();
        cache
            .append(vec![CopilotEntry {
                enabled: true,
                stage_name: "SET-1".to_string(),
                title: Some("批量作业".to_string()),
                is_raid: false,
                source: CopilotEntrySource::Local { path: batch_path },
                origin: CopilotOrigin::Set {
                    id: 1,
                    name: Some("测试作业集".to_string()),
                },
            }])
            .unwrap();
        for (stage_name, source) in [("TO-1", first_path), ("TO-2", second_path)] {
            cache
                .replace_current_single(Some(CopilotEntry {
                    enabled: true,
                    stage_name: stage_name.to_string(),
                    title: None,
                    is_raid: false,
                    source: CopilotEntrySource::Local { path: source },
                    origin: CopilotOrigin::Single,
                }))
                .unwrap();
        }

        assert_eq!(cache.current_single().unwrap().stage_name, "TO-2");
        assert_eq!(cache.entries().len(), 1);
        assert_eq!(cache.entries()[0].stage_name, "SET-1");
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
