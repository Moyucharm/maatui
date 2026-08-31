use super::*;
use crate::copilot::{CopilotEntry, CopilotEntrySource, CopilotOrigin};
use crate::roguelike::{RoguelikeConfig, RoguelikeField, RoguelikeSection, RoguelikeTheme};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

fn hot_update_env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn test_copilot_cache(count: usize) -> (PathBuf, CopilotCache) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("maatui-app-copilot-{unique}"));
    let path = dir.join("copilot-set.json");
    let files = dir.join("files");
    fs::create_dir_all(&files).unwrap();
    let mut cache = CopilotCache::load(path, files).unwrap();
    cache
        .append(
            (0..count)
                .map(|index| {
                    let file = dir.join(format!("{}.json", index + 1));
                    fs::write(&file, "{}").unwrap();
                    CopilotEntry {
                        enabled: true,
                        stage_name: format!("TO-{}", index + 1),
                        title: None,
                        is_raid: false,
                        source: CopilotEntrySource::Local { path: file },
                        origin: CopilotOrigin::Single,
                    }
                })
                .collect(),
        )
        .unwrap();
    (dir, cache)
}

fn test_current_single_cache() -> (PathBuf, CopilotCache) {
    let (dir, mut cache) = test_copilot_cache(0);
    let file = dir.join("single.json");
    fs::write(&file, "{}").unwrap();
    cache
        .replace_current_single(Some(CopilotEntry {
            enabled: true,
            stage_name: "TO-1".to_string(),
            title: Some("当前单作业".to_string()),
            is_raid: false,
            source: CopilotEntrySource::Local { path: file },
            origin: CopilotOrigin::Single,
        }))
        .unwrap();
    (dir, cache)
}

fn test_copilot_cache_with_set(single_count: usize, set_count: usize) -> (PathBuf, CopilotCache) {
    let (dir, mut cache) = test_copilot_cache(single_count);
    let entries = (0..set_count)
        .map(|index| {
            let file = dir.join(format!("set-{}.json", index + 1));
            fs::write(&file, "{}").unwrap();
            CopilotEntry {
                enabled: true,
                stage_name: format!("SET-{}", index + 1),
                title: Some(format!("作业集条目 {}", index + 1)),
                is_raid: false,
                source: CopilotEntrySource::Local { path: file },
                origin: CopilotOrigin::Set {
                    id: 50501,
                    name: Some("测试作业集".to_string()),
                },
            }
        })
        .collect();
    cache.append(entries).unwrap();
    (dir, cache)
}

fn test_config() -> (PathBuf, DailyConfig) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("maatui-app-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("daily.json");
    fs::write(&path, r#"{"tasks":[{"type":"StartUp"},{"type":"Award"}]}"#).unwrap();
    let config = DailyConfig::load(&path).unwrap();
    (dir, config)
}

fn test_running_command(kind: TaskKind, label: &str) -> TaskCommand {
    TaskCommand {
        kind,
        label: label.to_string(),
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        envs: Vec::new(),
        daily_single_index: None,
        cleanup: None,
    }
}

fn test_roguelike_config() -> (PathBuf, RoguelikeConfig) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("maatui-app-roguelike-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let config = RoguelikeConfig::load(dir.join("roguelike.json")).unwrap();
    (dir, config)
}

#[test]
fn roguelike_menu_navigation_and_tabs_are_independent() {
    let mut app = App::new();
    app.main_idx = 2;
    app.handle_main_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.screen, Screen::Roguelike);
    assert_eq!(app.roguelike_section(), RoguelikeSection::Control);

    app.handle_roguelike_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.roguelike_section(), RoguelikeSection::Advanced);
    app.handle_roguelike_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.roguelike_section(), RoguelikeSection::Control);
    app.handle_roguelike_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, Screen::Main);
}

#[test]
fn roguelike_field_editing_persists_and_rolls_back_on_failure() {
    let (dir, config) = test_roguelike_config();
    let mut app = App::new();
    app.roguelike_config = Some(config);
    app.roguelike = app.roguelike_config.as_ref().unwrap().options().clone();
    app.screen = Screen::Roguelike;
    app.roguelike_idx = 6;
    app.handle_roguelike_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.input.as_ref().map(|input| &input.target),
        Some(InputTarget::RoguelikeField(RoguelikeField::CoreChar))
    ));
    app.input.as_mut().unwrap().value = "维什戴尔".to_string();
    app.handle_input_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.roguelike.core_char, "维什戴尔");
    assert_eq!(
        RoguelikeConfig::load(dir.join("roguelike.json"))
            .unwrap()
            .options()
            .core_char,
        "维什戴尔"
    );

    let blocker = dir.join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    app.roguelike_config = Some(RoguelikeConfig::load(blocker.join("roguelike.json")).unwrap());
    let original = app.roguelike.clone();
    assert!(
        app.set_roguelike_field_result(
            RoguelikeField::Theme,
            FieldValue::String("Mizuki".to_string()),
        )
        .is_err()
    );
    assert_eq!(app.roguelike, original);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn roguelike_theme_options_keep_core_mode_contract() {
    let options = crate::roguelike::RoguelikeOptions {
        theme: RoguelikeTheme::Sami,
        mode: 5,
        ..Default::default()
    };
    assert!(
        options
            .visible_fields(RoguelikeSection::Advanced)
            .contains(&RoguelikeField::ExpectedCollapsalParadigms)
    );
    assert!(
        !options
            .visible_fields(RoguelikeSection::Advanced)
            .contains(&RoguelikeField::RefreshTraderWithDice)
    );
}

#[test]
fn menu_logs_are_isolated_and_preserved() {
    let mut app = App::new();
    app.screen = Screen::Daily;
    app.push_log(LogLevel::Info, "daily");
    app.screen = Screen::Copilot;
    app.push_log(LogLevel::Info, "copilot");
    app.screen = Screen::Roguelike;
    app.push_log(LogLevel::Info, "roguelike");
    app.screen = Screen::Update;
    app.push_log(LogLevel::Info, "update");

    assert_eq!(app.log_buffer(LogScope::Daily).lines.len(), 1);
    assert_eq!(app.log_buffer(LogScope::Copilot).lines.len(), 1);
    assert_eq!(app.log_buffer(LogScope::Roguelike).lines.len(), 1);
    assert_eq!(app.log_buffer(LogScope::Update).lines.len(), 1);
    assert_eq!(app.log_buffer(LogScope::Daily).lines[0].text, "daily");
    assert_eq!(app.log_buffer(LogScope::Copilot).lines[0].text, "copilot");
    assert_eq!(
        app.log_buffer(LogScope::Roguelike).lines[0].text,
        "roguelike"
    );
    assert_eq!(app.log_buffer(LogScope::Update).lines[0].text, "update");
}

#[test]
fn starting_task_clears_only_its_log_scope() {
    let mut app = App::new();
    let scopes = [
        LogScope::Daily,
        LogScope::Copilot,
        LogScope::Roguelike,
        LogScope::Update,
    ];
    for (kind, scope, label) in [
        (TaskKind::Daily, LogScope::Daily, "测试每日任务"),
        (TaskKind::Copilot, LogScope::Copilot, "测试自动战斗"),
        (TaskKind::Roguelike, LogScope::Roguelike, "测试自动肉鸽"),
        (TaskKind::Update, LogScope::Update, "测试更新任务"),
    ] {
        let old = format!("旧日志 · {label}");
        app.push_log_to(scope, LogLevel::Info, old.clone());
        for other_scope in scopes {
            if other_scope != scope {
                app.push_log_to(other_scope, LogLevel::Info, format!("保留日志 · {label}"));
            }
        }

        assert!(app.start_command(test_running_command(kind, label)));
        let lines = &app.log_buffer(scope).lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "启动: /bin/sh -c sleep 30");
        assert!(lines.iter().all(|line| line.text != old));
        for other_scope in scopes {
            if other_scope != scope {
                assert!(
                    app.log_buffer(other_scope)
                        .lines
                        .iter()
                        .any(|line| line.text == format!("保留日志 · {label}"))
                );
            }
        }
        app.cleanup();
    }

    app.push_log_to(LogScope::Daily, LogLevel::Success, "第一轮终态");
    assert!(app.start_command(test_running_command(TaskKind::Daily, "第二轮每日任务")));
    assert!(
        app.log_buffer(LogScope::Daily)
            .lines
            .iter()
            .all(|line| line.text != "第一轮终态")
    );
    app.cleanup();
}

#[test]
fn background_copilot_import_does_not_override_or_pollute_daily_run() {
    let mut app = App::new();
    for scope in [
        LogScope::Daily,
        LogScope::Copilot,
        LogScope::Roguelike,
        LogScope::Update,
    ] {
        app.log_buffer_mut(scope).lines.clear();
    }
    app.screen = Screen::Daily;
    app.phase = TaskPhase::Running;
    app.active_log_scope = LogScope::Daily;
    app.status_text = "每日任务运行中".to_string();
    app.copilot_importing = true;

    app.copilot_import_tx
        .send(CopilotImportEvent::Progress(
            crate::copilot::ImportProgress {
                completed: 1,
                total: 2,
                label: "后台导入".to_string(),
            },
        ))
        .unwrap();
    app.copilot_import_tx
        .send(CopilotImportEvent::Finished {
            destination: ImportDestination::CurrentSingle,
            result: Err("tile-pos t0-1 导入失败".to_string()),
        })
        .unwrap();
    app.poll_copilot_import();

    assert_eq!(app.status_text, "每日任务运行中");
    assert!(app.log_buffer(LogScope::Daily).lines.is_empty());
    assert!(
        app.log_buffer(LogScope::Copilot)
            .lines
            .iter()
            .any(|line| line.text.contains("tile-pos t0-1"))
    );
    assert!(!app.saw_ocr_to_t0_error);
    assert!(!app.saw_outdated_resource_error);
}

#[test]
fn menu_log_capacity_is_enforced_per_scope() {
    let mut app = App::new();
    for index in 0..MAX_LOG_LINES + 5 {
        app.push_log_to(LogScope::Daily, LogLevel::Info, index.to_string());
    }
    app.push_log_to(LogScope::Copilot, LogLevel::Info, "copilot");

    assert_eq!(app.log_buffer(LogScope::Daily).lines.len(), MAX_LOG_LINES);
    assert_eq!(app.log_buffer(LogScope::Daily).lines[0].text, "5");
    assert_eq!(app.log_buffer(LogScope::Copilot).lines.len(), 1);
}

#[test]
fn commands_select_their_own_log_scope() {
    assert_eq!(
        log_scope_for_command(&TaskCommand::daily()),
        LogScope::Daily
    );
    assert_eq!(
        log_scope_for_command(&TaskCommand::copilot(vec!["run".to_string()])),
        LogScope::Copilot
    );
    assert_eq!(
        log_scope_for_command(&TaskCommand::resource_update()),
        LogScope::Update
    );
    assert_eq!(
        log_scope_for_command(&TaskCommand::core_update()),
        LogScope::Update
    );
    let roguelike = TaskCommand::roguelike(
        "maatui-roguelike-test",
        std::path::PathBuf::from("/tmp/roguelike.json"),
    );
    assert_eq!(log_scope_for_command(&roguelike), LogScope::Roguelike);
}

#[test]
fn config_is_nested_under_daily_navigation() {
    assert_eq!(
        MainMenuItem::ALL.map(MainMenuItem::label),
        ["每日任务", "自动战斗", "自动肉鸽", "更新管理", "退出"]
    );
    let mut app = App::new();
    app.screen = Screen::Config;
    app.handle_config_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, Screen::Daily);
}

#[test]
fn daily_progress_uses_config_order_and_total() {
    let mut app = App::new();
    app.daily_run = Some(DailyRunState {
        tasks: vec![
            TaskSummary {
                name: "启动游戏".to_string(),
                task_type: "StartUp".to_string(),
                enabled: true,
            },
            TaskSummary {
                name: "关闭游戏".to_string(),
                task_type: "CloseDown".to_string(),
                enabled: false,
            },
        ],
        total: 1,
    });

    app.on_daily_task_started(2, "CloseDown");

    assert_eq!(
        app.run_progress,
        Some(RunProgress {
            current: 1,
            total: 1,
            label: "关闭游戏".to_string(),
        })
    );
}

#[test]
fn daily_progress_matches_label_by_taskchain_when_disabled_in_between() {
    let mut app = App::new();
    app.daily_run = Some(DailyRunState {
        tasks: vec![
            TaskSummary {
                name: "启动游戏".to_string(),
                task_type: "StartUp".to_string(),
                enabled: true,
            },
            TaskSummary {
                name: "刷理智".to_string(),
                task_type: "Fight".to_string(),
                enabled: false,
            },
            TaskSummary {
                name: "公招".to_string(),
                task_type: "Recruit".to_string(),
                enabled: true,
            },
        ],
        total: 2,
    });

    // taskid=2 对应实际执行的第二个启用任务「公招」，
    // 不应错位显示为中间关闭的「刷理智」。
    app.on_daily_task_started(2, "Recruit");

    assert_eq!(
        app.run_progress,
        Some(RunProgress {
            current: 2,
            total: 2,
            label: "公招".to_string(),
        })
    );
}

#[test]
fn daily_progress_distinguishes_enabled_tasks_with_same_type() {
    let mut app = App::new();
    app.daily_run = Some(DailyRunState {
        tasks: vec![
            TaskSummary {
                name: "常驻关卡".to_string(),
                task_type: "Fight".to_string(),
                enabled: true,
            },
            TaskSummary {
                name: "活动关卡".to_string(),
                task_type: "Fight".to_string(),
                enabled: true,
            },
        ],
        total: 2,
    });

    app.on_daily_task_started(2, "Fight");

    assert_eq!(
        app.run_progress,
        Some(RunProgress {
            current: 2,
            total: 2,
            label: "活动关卡".to_string(),
        })
    );
}

#[test]
fn complete_spinner_contains_full_cycle() {
    let app = App::new();
    let frame = app.spinner();
    assert!(['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'].contains(&frame));
}

#[test]
fn failed_move_keeps_selection_index() {
    let (dir, config) = test_config();
    let mut app = App::new();
    app.config = Some(config);
    app.screen = Screen::Config;
    app.config_idx = 1;
    fs::remove_file(dir.join("daily.json")).unwrap();
    fs::remove_dir(&dir).unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));

    assert_eq!(app.config_idx, 1);
    assert_eq!(
        app.config.as_ref().unwrap().task_type(0).as_deref(),
        Some("StartUp")
    );
}

#[test]
fn successful_config_operation_clears_failed_state() {
    let (dir, config) = test_config();
    let mut app = App::new();
    app.config = Some(config);
    app.config_save_worker = Some(config_save::ConfigSaveWorker::new().unwrap());
    app.last_failed = true;

    assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));
    assert!(!app.last_failed);
    app.cleanup();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn daily_command_flushes_latest_config_before_starting() {
    let (dir, config) = test_config();
    let mut app = App::new();
    app.config = Some(config);
    app.screen = Screen::Daily;
    app.config_save_worker = Some(config_save::ConfigSaveWorker::new().unwrap());

    assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));
    assert!(app.config_dirty);
    assert!(app.start_command(test_running_command(TaskKind::Daily, "测试每日任务",)));

    assert_eq!(
        DailyConfig::load(&dir.join("daily.json"))
            .unwrap()
            .task_enabled(0),
        Some(false)
    );
    assert!(!app.config_dirty);
    app.cleanup();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn daily_command_does_not_start_after_save_failure() {
    let dir = test_config().0;
    let outside = std::env::temp_dir().join(format!(
        "maatui-app-daily-save-failure-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&outside).unwrap();
    let target = outside.join("daily.json");
    fs::write(&target, r#"{"tasks":[{"type":"StartUp"}]}"#).unwrap();
    let path = dir.join("daily.json");
    fs::remove_file(&path).unwrap();
    symlink(&target, &path).unwrap();

    let mut app = App::new();
    app.config = Some(DailyConfig::load(&path).unwrap());
    app.screen = Screen::Daily;
    app.config_save_worker = Some(config_save::ConfigSaveWorker::new().unwrap());
    assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));

    assert!(!app.start_command(TaskCommand::daily()));
    assert!(app.task.is_none());
    assert!(app.config_dirty);
    assert!(app.log_buffer(LogScope::Daily).lines.iter().any(|line| {
        line.level == LogLevel::Error && line.text.contains("后台保存 daily 配置失败")
    }));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        r#"{"tasks":[{"type":"StartUp"}]}"#
    );
    assert!(!app.reload_config());
    assert_eq!(app.config.as_ref().unwrap().task_enabled(0), Some(false));

    app.cleanup();
    fs::remove_dir_all(dir).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn reload_waits_for_pending_config_save() {
    let (dir, config) = test_config();
    let path = dir.join("daily.json");
    let mut app = App::new();
    app.config = Some(config);
    app.screen = Screen::Config;
    app.config_save_worker = Some(config_save::ConfigSaveWorker::new().unwrap());
    assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));
    assert!(app.reload_config_from(path));
    assert_eq!(app.config.as_ref().unwrap().task_enabled(0), Some(false));
    assert!(!app.config_dirty);
    app.cleanup();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn config_save_error_keeps_dirty_state_and_is_visible() {
    let mut app = App::new();
    app.screen = Screen::Config;
    app.config_revision = 7;

    app.report_config_save_failure("序列化 daily 配置失败: 测试错误".to_string());

    assert!(app.config_dirty);
    assert_eq!(app.config_failed_revision, Some(7));
    assert!(app.last_failed);
    assert!(app.status_text.contains("序列化 daily 配置失败"));
    assert!(app.log_buffer(LogScope::Daily).lines.iter().any(|line| {
        line.level == LogLevel::Error && line.text.contains("序列化 daily 配置失败")
    }));
}

#[test]
fn cleanup_reports_final_config_save_failure_without_clearing_dirty() {
    let dir = test_config().0;
    let outside = std::env::temp_dir().join(format!(
        "maatui-app-cleanup-save-failure-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&outside).unwrap();
    let target = outside.join("daily.json");
    fs::write(&target, r#"{"tasks":[{"type":"StartUp"}]}"#).unwrap();
    let path = dir.join("daily.json");
    fs::remove_file(&path).unwrap();
    symlink(&target, &path).unwrap();

    let mut app = App::new();
    app.config = Some(DailyConfig::load(&path).unwrap());
    app.screen = Screen::Config;
    app.config_save_worker = Some(config_save::ConfigSaveWorker::new().unwrap());
    assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));

    app.cleanup();

    assert!(app.config_save_worker.is_none());
    assert!(app.config_dirty);
    assert!(app.log_buffer(LogScope::Daily).lines.iter().any(|line| {
        line.level == LogLevel::Error && line.text.contains("后台保存 daily 配置失败")
    }));
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        r#"{"tasks":[{"type":"StartUp"}]}"#
    );
    fs::remove_dir_all(dir).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn current_single_preserves_and_switches_both_modes() {
    let (dir, cache) = test_copilot_cache(0);
    let files = cache.files_dir().to_path_buf();
    let path = dir.join("both.json");
    fs::write(
        &path,
        r#"{"stage_name":"TO-1","difficulty":3,"doc":{"title":"双模式"}}"#,
    )
    .unwrap();
    let report = import_source(path.to_str().unwrap(), ImportKind::Single, &files, |_| {}).unwrap();
    assert_eq!(
        report
            .entries
            .iter()
            .map(|entry| entry.is_raid)
            .collect::<Vec<_>>(),
        vec![false, true]
    );
    let mut app = App::new();
    app.copilot_cache = Some(cache);

    app.finish_copilot_import(ImportDestination::CurrentSingle, report)
        .unwrap();

    let cache = app.copilot_cache.as_ref().unwrap();
    assert_eq!(
        cache.current_single_supported_modes().unwrap(),
        (true, true)
    );
    assert!(!cache.current_single().unwrap().is_raid);
    assert_eq!(cache.entries_len(), 0);

    app.handle_single_copilot_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .current_single()
            .unwrap()
            .is_raid
    );
    let command = app.current_single_copilot_command().unwrap();
    assert!(
        command
            .args
            .windows(2)
            .any(|args| args == ["--raid", "raid"])
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn input_clear_and_batch_import_type_switch_keep_text() {
    let mut app = App::new();
    app.input = Some(InputDialog {
        title: "添加到作业集".to_string(),
        value: "prts://123".to_string(),
        target: InputTarget::CopilotAdd {
            kind: ImportKind::Set,
            destination: ImportDestination::Batch,
        },
    });

    app.handle_input_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    app.handle_input_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.input.as_ref().unwrap().value, "prts://123");
    assert!(matches!(
        app.input.as_ref().map(|input| &input.target),
        Some(InputTarget::CopilotAdd {
            kind: ImportKind::Single,
            destination: ImportDestination::Batch,
        })
    ));

    app.handle_input_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_input_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.input.as_ref().unwrap().value, "prts://123");
    assert!(matches!(
        app.input.as_ref().map(|input| &input.target),
        Some(InputTarget::CopilotAdd {
            kind: ImportKind::Set,
            destination: ImportDestination::Batch,
        })
    ));

    app.handle_input_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert!(app.input.as_ref().unwrap().value.is_empty());
}

#[test]
fn input_question_mark_stays_in_text_and_does_not_open_help() {
    let mut app = App::new();
    app.input = Some(InputDialog {
        title: "输入".to_string(),
        value: String::new(),
        target: InputTarget::CopilotText(CopilotTextField::SupportName),
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));

    assert_eq!(app.input.as_ref().unwrap().value, "?");
    assert!(!app.shortcut_help_open);
}

#[test]
fn task_edit_variants_accepts_enter_and_e() {
    let variants_index = EditorSection::ALL
        .iter()
        .position(|section| *section == EditorSection::Variants)
        .unwrap();

    for code in [KeyCode::Enter, KeyCode::Char('e')] {
        let mut app = App::new();
        app.screen = Screen::TaskEdit;
        app.section_idx = variants_index;

        app.handle_task_edit_key(KeyEvent::new(code, KeyModifiers::NONE));

        assert_eq!(app.screen, Screen::VariantList);
        assert_eq!(app.variant_idx, 0);
    }
}

#[test]
fn single_import_in_progress_reports_without_opening_input() {
    let mut app = App::new();
    app.copilot_importing = true;

    app.handle_single_copilot_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));

    assert!(app.input.is_none());
    assert_eq!(app.status_text, "已有作业正在导入，请稍候");
}

#[test]
fn config_move_boundaries_do_not_replace_header_status() {
    let (dir, config) = test_config();
    let mut app = App::new();
    app.config = Some(config);
    app.screen = Screen::Config;
    app.status_text = "就绪".to_string();

    app.config_idx = 0;
    app.handle_config_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert_eq!(app.status_text, "就绪");

    app.config_idx = 1;
    app.handle_config_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.status_text, "就绪");

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_single_import_appends_normal_then_raid_at_end() {
    let (dir, cache) = test_current_single_cache();
    let files = cache.files_dir().to_path_buf();
    let existing_path = dir.join("existing.json");
    fs::write(&existing_path, "{}").unwrap();
    let mut cache = cache;
    cache
        .append(vec![CopilotEntry {
            enabled: true,
            stage_name: "OLD-1".to_string(),
            title: None,
            is_raid: false,
            source: CopilotEntrySource::Local {
                path: existing_path,
            },
            origin: CopilotOrigin::Set {
                id: 1,
                name: Some("原列表".to_string()),
            },
        }])
        .unwrap();
    let path = dir.join("both.json");
    fs::write(
        &path,
        r#"{"stage_name":"TO-1","difficulty":3,"doc":{"title":"双模式"}}"#,
    )
    .unwrap();
    let report = import_source(path.to_str().unwrap(), ImportKind::Single, &files, |_| {}).unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);

    app.finish_copilot_import(ImportDestination::Batch, report)
        .unwrap();

    let cache = app.copilot_cache.as_ref().unwrap();
    assert_eq!(cache.entries_len(), 3);
    assert_eq!(cache.entry(0).unwrap().stage_name, "OLD-1");
    assert_eq!(
        cache
            .entries()
            .iter()
            .map(|entry| (entry.stage_name.as_str(), entry.is_raid))
            .collect::<Vec<_>>(),
        vec![("OLD-1", false), ("TO-1", false), ("TO-1", true)]
    );
    assert!(cache.current_single().is_some());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_toggle_all_and_clear_follow_requested_rules() {
    let (dir, mut cache) = test_copilot_cache(3);
    let single_path = dir.join("current.json");
    fs::write(&single_path, "{}").unwrap();
    cache
        .replace_current_single(Some(CopilotEntry {
            enabled: true,
            stage_name: "CURRENT-1".to_string(),
            title: None,
            is_raid: false,
            source: CopilotEntrySource::Local { path: single_path },
            origin: CopilotOrigin::Single,
        }))
        .unwrap();
    cache.toggle(1).unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 1;

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.copilot_cache.as_ref().unwrap().enabled_count(), 0);
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
    assert_eq!(app.copilot_cache.as_ref().unwrap().enabled_count(), 3);
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(matches!(
        app.confirm.as_ref().map(|confirm| &confirm.target),
        Some(ConfirmTarget::ClearCopilotEntries)
    ));
    app.handle_confirm_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.copilot_cache.as_ref().unwrap().entries_len(), 0);
    assert!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .current_single()
            .is_some()
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn option_fields_display_labels_and_preserve_unknown_values() {
    let series = task_fields("Fight", false)
        .into_iter()
        .find(|field| field.key == "series")
        .unwrap();
    let drones = task_fields("Infrast", false)
        .into_iter()
        .find(|field| field.key == "drones")
        .unwrap();
    let mode = task_fields("Infrast", false)
        .into_iter()
        .find(|field| field.key == "mode")
        .unwrap();
    let formation = task_fields("Copilot", true)
        .into_iter()
        .find(|field| field.key == "formation_index")
        .unwrap();
    let facility = task_fields("Infrast", false)
        .into_iter()
        .find(|field| field.key == "facility")
        .unwrap();

    assert_eq!(series.display_value(&FieldValue::Integer(0)), "AUTO");
    assert_eq!(drones.display_value(&string("Money")), "贸易站：龙门币");
    assert_eq!(mode.display_value(&FieldValue::Integer(0)), "默认模式");
    assert_eq!(formation.display_value(&FieldValue::Integer(0)), "当前编队");
    assert_eq!(
        facility.display_value(&FieldValue::StringArray(vec![
            "Mfg".into(),
            "Unknown".into()
        ])),
        "制造站, Unknown"
    );
    assert_eq!(drones.display_value(&string("Custom")), "Custom");
    assert!(validate_field_value("series", &FieldValue::Integer(7)).is_err());
    assert_eq!(
        series_options()
            .into_iter()
            .filter_map(|option| match option.value {
                FieldValue::Integer(value) => Some(value),
                _ => None,
            })
            .max(),
        Some(6)
    );
}

#[test]
fn copilot_tabs_use_contextual_add_and_real_indices() {
    let (dir, cache) = test_copilot_cache_with_set(2, 2);
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.screen = Screen::Copilot;

    app.handle_single_copilot_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(matches!(
        app.input.as_ref().map(|dialog| &dialog.target),
        Some(InputTarget::CopilotAdd {
            kind: ImportKind::Single,
            destination: ImportDestination::CurrentSingle,
        })
    ));
    app.input = None;

    app.copilot_section_idx = 1;
    app.copilot_idx = 0;
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(matches!(
        app.input.as_ref().map(|dialog| &dialog.target),
        Some(InputTarget::CopilotAdd {
            kind: ImportKind::Set,
            destination: ImportDestination::Batch,
        })
    ));
    app.input = None;

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(
        !app.copilot_cache
            .as_ref()
            .unwrap()
            .entry(0)
            .unwrap()
            .enabled
    );
    assert!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .entry(1)
            .unwrap()
            .enabled
    );

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.copilot_idx, 1);
    assert_eq!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .entry(0)
            .unwrap()
            .stage_name,
        "TO-2"
    );
    assert_eq!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .entry(1)
            .unwrap()
            .stage_name,
        "TO-1"
    );

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(matches!(
        app.confirm.as_ref().map(|dialog| &dialog.target),
        Some(ConfirmTarget::DeleteCopilot(1))
    ));
    app.handle_confirm_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(app.copilot_cache.as_ref().unwrap().len(), 3);
    assert_eq!(app.copilot_idx, 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copilot_avatar_cache_clear_requires_confirmation_and_idle_task() {
    let mut app = App::new();
    app.copilot_section_idx = 2;
    app.copilot_settings_idx = 8;

    app.handle_copilot_settings_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.confirm.as_ref().map(|dialog| &dialog.target),
        Some(ConfirmTarget::ClearAvatarCache)
    ));

    app.phase = TaskPhase::Running;
    app.handle_confirm_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.confirm.is_none());
    assert_eq!(app.status_text, "错误: 任务运行中，无法清理干员头像缓存");
    assert!(app.last_failed);
}

#[test]
fn copilot_tab_navigation_clamps_selection() {
    let (dir, cache) = test_copilot_cache_with_set(0, 1);
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.screen = Screen::Copilot;
    app.copilot_idx = 2;

    app.handle_copilot_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.copilot_section(), CopilotSection::Sets);
    assert_eq!(app.copilot_idx, 0);
    app.handle_copilot_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.copilot_section(), CopilotSection::Settings);
    app.handle_copilot_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.copilot_section(), CopilotSection::Singles);
    app.handle_copilot_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.copilot_section(), CopilotSection::Singles);
    app.handle_copilot_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.copilot_section(), CopilotSection::Sets);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copilot_empty_set_keeps_low_value_actions_silent() {
    let (dir, cache) = test_copilot_cache(0);
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 1;

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.status_text, "就绪");
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert_eq!(app.status_text, "作业集为空，请先添加作业");

    assert!(app.confirm.is_none());
    assert!(app.copilot_detail.is_none());
    assert_eq!(app.phase, TaskPhase::Idle);
    assert_eq!(app.copilot_cache.as_ref().unwrap().len(), 0);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copilot_move_boundaries_do_not_replace_header_status() {
    let (dir, cache) = test_copilot_cache(1);
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 1;
    app.status_text = "就绪".to_string();

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert_eq!(app.status_text, "就绪");
    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.status_text, "就绪");

    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copilot_settings_run_reports_when_no_entries_are_enabled() {
    let (dir, mut cache) = test_copilot_cache(1);
    cache.toggle(0).unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 2;

    app.handle_copilot_settings_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert_eq!(app.status_text, "没有启用的作业");
    assert_eq!(app.phase, TaskPhase::Idle);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shortcut_help_opens_scrolls_and_closes_without_quitting() {
    let mut app = App::new();
    app.screen = Screen::Copilot;

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(app.shortcut_help_open);
    assert_eq!(app.shortcut_help_scroll, 0);

    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.shortcut_help_scroll, 10);

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.shortcut_help_open);
    assert_eq!(app.shortcut_help_scroll, 0);
    assert_eq!(app.screen, Screen::Copilot);
    assert!(!app.should_quit);
}

#[test]
fn copilot_detail_reads_operator_requirements_and_notes() {
    let (dir, cache) = test_current_single_cache();
    let path = cache.current_single_path().unwrap();
    fs::write(
        &path,
        r#"{
                "version": 3,
                "stage_name": "TO-1",
                "doc": {"title": "详情测试", "details": "第一行\n第二行"},
                "opers": [{
                    "name": "逻各斯",
                    "skill": 3,
                    "skill_usage": 1,
                    "requirements": {"elite": 2, "skill_level": 10, "module": 4}
                }],
                "groups": [{
                    "name": "替补",
                    "opers": [{"name": "阿米娅", "skill": 2, "requirements": {"module": 1}}]
                }],
                "actions": [{"type": "SkillUsage", "name": "逻各斯", "skill_times": 2}]
            }"#,
    )
    .unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.screen = Screen::Copilot;

    app.open_current_single_detail();

    let detail = app.copilot_detail.as_ref().unwrap();
    assert!(detail.title.starts_with("单作业详情"));
    let text = detail.lines.join("\n");
    assert!(text.contains("关卡名：TO-1"));
    assert!(text.contains("逻各斯 · 技能 3"));
    assert!(text.contains("模组 4"));
    assert!(text.contains("替补：阿米娅（技能 2"));
    assert!(text.contains("第一行"));
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.copilot_detail.as_ref().unwrap().scroll, 10);
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.copilot_detail.is_none());
    assert_eq!(app.screen, Screen::Copilot);
    assert!(!app.should_quit);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn set_tab_opens_set_entry_detail_title() {
    let (dir, cache) = test_copilot_cache_with_set(0, 1);
    let path = cache.resolved_file_path(0).unwrap();
    fs::write(&path, r#"{"stage_name":"SET-1","opers":[]}"#).unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 1;

    app.handle_copilot_list_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert!(
        app.copilot_detail
            .as_ref()
            .unwrap()
            .title
            .starts_with("作业集条目详情")
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn selected_set_entry_also_uses_single_copilot_command() {
    let (dir, cache) = test_copilot_cache_with_set(0, 1);
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_section_idx = 1;

    let command = app.selected_copilot_command().unwrap();

    assert_eq!(command.args[0], "copilot");
    assert!(command.args[1].contains("set-1.json"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn selected_copilot_command_uses_item_difficulty_without_toggling_it() {
    let (dir, mut cache) = test_copilot_cache(0);
    let file = dir.join("raid.json");
    fs::write(&file, "{}").unwrap();
    cache
        .replace_current_single(Some(CopilotEntry {
            enabled: true,
            stage_name: "TO-1".to_string(),
            title: None,
            is_raid: true,
            source: CopilotEntrySource::Local { path: file },
            origin: CopilotOrigin::Single,
        }))
        .unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot.formation = true;
    app.copilot.loop_times = 3;
    let command = app.current_single_copilot_command().unwrap();

    assert_eq!(command.args[0], "copilot");
    assert!(
        command
            .args
            .windows(2)
            .any(|args| args == ["--raid", "raid"])
    );
    assert!(command.args.iter().any(|arg| arg == "--formation"));
    assert!(
        command
            .args
            .windows(2)
            .any(|args| args == ["--loop-times", "3"])
    );
    assert!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .current_single()
            .is_some()
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_task_keeps_cross_tab_enabled_order() {
    let (dir, mut cache) = test_copilot_cache_with_set(2, 2);
    cache.toggle(1).unwrap();
    let options = CopilotRunOptions {
        formation_index: 0,
        use_sanity_potion: false,
        add_trust: false,
        ignore_requirements: false,
        support_unit_usage: 0,
        support_unit_name: String::new(),
    };

    let indices = cache.enabled_indices();
    let task =
        crate::copilot::write_batch_task_in(&cache, indices.clone(), &options, &dir).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&task.path).unwrap()).unwrap();
    let stages = value
        .pointer("/tasks/0/params/copilot_list")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["stage_name"].as_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(indices, vec![0, 2, 3]);
    assert_eq!(stages, vec!["TO-1", "SET-1", "SET-2"]);
    remove_batch_task(&task.path);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_progress_reports_current_item() {
    let (dir, cache) = test_copilot_cache_with_set(1, 2);
    let task_path = dir.join("batch.json");
    fs::write(&task_path, "{}").unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_batch = Some(CopilotBatchState::new(task_path, vec![0, 1, 2]));
    app.copilot_batch.as_mut().unwrap().mark_success();

    let (current, total, name) = app.copilot_batch_progress().unwrap();

    assert_eq!((current, total), (2, 3));
    assert!(name.contains("SET-1"));
    app.copilot_batch.as_mut().unwrap().mark_success();
    app.copilot_batch.as_mut().unwrap().mark_success();
    let (current, total, name) = app.copilot_batch_progress().unwrap();
    assert_eq!((current, total), (3, 3));
    assert!(name.contains("收尾"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_failure_keeps_failed_and_remaining_entries_enabled() {
    let (dir, cache) = test_copilot_cache(3);
    let task_path = dir.join("batch.json");
    fs::write(&task_path, "{}").unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_batch = Some(CopilotBatchState::new(task_path.clone(), vec![0, 1, 2]));
    app.active_label = "作业集自动战斗".to_string();

    app.on_copilot_stage_succeeded();
    app.on_copilot_stage_succeeded();
    app.on_exited(Some(1), false);

    let cache = app.copilot_cache.as_ref().unwrap();
    assert!(!cache.entry(0).unwrap().enabled);
    assert!(!cache.entry(1).unwrap().enabled);
    assert!(cache.entry(2).unwrap().enabled);
    assert!(!task_path.exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batch_success_reconciles_missed_progress_events() {
    let (dir, cache) = test_copilot_cache(3);
    let task_path = dir.join("batch.json");
    fs::write(&task_path, "{}").unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_batch = Some(CopilotBatchState::new(task_path, vec![0, 1, 2]));
    app.active_label = "作业集自动战斗".to_string();
    app.active_log_scope = LogScope::Copilot;

    app.on_copilot_stage_succeeded();
    app.on_exited(Some(0), false);

    assert!(
        app.copilot_cache
            .as_ref()
            .unwrap()
            .entries()
            .iter()
            .all(|entry| !entry.enabled)
    );
    assert!(
        app.log_buffer(LogScope::Copilot)
            .lines
            .iter()
            .any(|log| log.text.contains("已补记 2 个"))
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stopped_batch_only_disables_confirmed_successes() {
    let (dir, cache) = test_copilot_cache(2);
    let task_path = dir.join("batch.json");
    fs::write(&task_path, "{}").unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    app.copilot_batch = Some(CopilotBatchState::new(task_path, vec![0, 1]));
    app.active_label = "作业集自动战斗".to_string();

    app.on_copilot_stage_succeeded();
    app.on_exited(None, true);

    let cache = app.copilot_cache.as_ref().unwrap();
    assert!(!cache.entry(0).unwrap().enabled);
    assert!(cache.entry(1).unwrap().enabled);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn internal_batch_abort_remains_failed_after_process_stops() {
    let (dir, cache) = test_copilot_cache(1);
    let task_path = dir.join("batch.json");
    fs::write(&task_path, "{}").unwrap();
    let mut app = App::new();
    app.copilot_cache = Some(cache);
    let mut batch = CopilotBatchState::new(task_path, vec![0]);
    batch.set_abort_error("进度监视失败".to_string());
    app.copilot_batch = Some(batch);
    app.active_label = "作业集自动战斗".to_string();
    app.active_log_scope = LogScope::Copilot;

    app.on_exited(None, true);

    assert!(app.last_failed);
    assert!(app.status_text.contains("失败"));
    assert!(
        app.log_buffer(LogScope::Copilot)
            .lines
            .iter()
            .any(|log| log.text == "进度监视失败")
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn manual_daily_stop_remains_successful_without_notification() {
    let mut app = App::new();
    app.active_label = "每日任务".to_string();
    app.active_task_kind = Some(TaskKind::Daily);
    app.phase = TaskPhase::Stopping;

    app.on_exited(None, true);

    assert!(!app.last_failed);
    assert!(app.status_text.contains("已停止"));
    assert!(app.active_task_kind.is_none());
    assert!(app.task_abort_error.is_none());
}

#[test]
fn internal_daily_abort_remains_failed_after_process_stops() {
    let mut app = App::new();
    app.active_label = "每日任务".to_string();
    app.active_log_scope = LogScope::Daily;
    app.phase = TaskPhase::Running;

    app.abort_running_task("进度监视失败".to_string());
    app.on_exited(None, true);

    assert!(app.last_failed);
    assert!(app.status_text.contains("失败"));
    assert!(
        app.log_buffer(LogScope::Daily)
            .lines
            .iter()
            .any(|log| log.text == "进度监视失败")
    );
    assert!(app.task_abort_error.is_none());
}

#[test]
fn unavailable_notification_worker_is_logged_without_affecting_task() {
    let mut app = App::new();
    app.active_log_scope = LogScope::Daily;
    app.notification_worker = None;

    app.send_task_notification(TaskKind::Daily, "每日任务", TaskOutcome::Completed);

    assert!(
        app.log_buffer(LogScope::Daily)
            .lines
            .iter()
            .any(|log| log.text.contains("通知线程不可用"))
    );
}

#[test]
fn fight_stage_keeps_fixed_and_activity_editors_available() {
    let fields = task_fields("Fight", false);
    let stage = fields.iter().find(|field| field.key == "stage").unwrap();
    assert!(matches!(
        stage.editor,
        FieldEditor::Stage { allow_custom: true }
    ));
    assert_eq!(stage.default, FieldValue::String(String::new()));
    assert!(validate_field_value("stage", &FieldValue::String(String::new())).is_ok());
    assert!(
        stage_options()
            .iter()
            .any(|option| { option.value == FieldValue::String("1-7".to_string()) })
    );
    assert!(
        stage_options()
            .iter()
            .any(|option| { option.value == FieldValue::String(String::new()) })
    );
}

#[test]
fn compound_conditions_are_preserved_as_read_only() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("maatui-app-condition-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("daily.json");
    fs::write(
            &path,
            r#"{"tasks":[{"type":"Fight","variants":[{"condition":{"type":"And","conditions":[{"type":"Time","start":"08:00:00"}]}}]}]}"#,
        )
        .unwrap();

    let mut app = App::new();
    app.config = Some(DailyConfig::load(&path).unwrap());
    app.screen = Screen::VariantEdit;
    app.config_idx = 0;
    app.variant_idx = 0;
    app.field_idx = 0;
    app.handle_variant_edit_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(app.select.is_none());
    assert!(app.input.is_none());
    assert!(app.status_text.contains("已保留原配置"));
    assert!(condition_type_options().iter().all(|option| {
        !matches!(
            &option.value,
            FieldValue::String(value) if is_compound_condition_type(value)
        )
    }));
    assert!(validate_field_value("type", &string("And")).is_err());

    app.field_idx = 2;
    app.handle_variant_edit_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.input.is_none());
    assert!(app.select.is_none());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stage_refresh_does_not_replace_running_or_failed_status() {
    let mut app = App::new();
    app.phase = TaskPhase::Running;
    app.status_text = "每日任务运行中".to_string();
    app.stage_refresh_tx
        .send(StageRefreshEvent::Updated {
            catalog: StageCatalog::default(),
            tasks_updated: true,
        })
        .unwrap();
    app.poll_stage_refresh();
    assert_eq!(app.status_text, "每日任务运行中");
    assert!(!app.last_failed);

    app.phase = TaskPhase::Idle;
    app.last_failed = true;
    app.status_text = "失败 · exit 1".to_string();
    app.stage_refresh_tx
        .send(StageRefreshEvent::Failed("network".to_string()))
        .unwrap();
    app.poll_stage_refresh();
    assert_eq!(app.status_text, "失败 · exit 1");
    assert!(app.last_failed);

    app.last_failed = false;
    app.status_text = "就绪".to_string();
    app.stage_refresh_tx
        .send(StageRefreshEvent::Updated {
            catalog: StageCatalog::default(),
            tasks_updated: true,
        })
        .unwrap();
    app.poll_stage_refresh();
    assert!(app.status_text.contains("热更新"));
    assert!(!app.stage_refreshing);
}

#[test]
fn repeated_stage_refresh_is_coalesced() {
    let mut app = App::new();
    app.stage_refreshing = true;
    app.status_text = "就绪".to_string();

    app.refresh_stage_catalog();

    assert!(app.stage_refreshing);
    assert_eq!(app.status_text, "活动关卡目录正在刷新…");
}

#[test]
fn detects_tile_pos_outdated_errors() {
    assert!(looks_like_outdated_resource_error(
        "Error: Failed to find Tile-Pos file for TO-1, your resources may be outdated"
    ));
    assert!(looks_like_outdated_resource_error("UnsupportedLevel"));
    assert!(!looks_like_outdated_resource_error(
        "Hot update completed successfully"
    ));
    assert!(looks_like_ocr_to_as_t0_error(
        "Error: Failed to find Tile-Pos file for T0-1, your resources may be outdated"
    ));
    assert!(!looks_like_ocr_to_as_t0_error(
        "Error: Failed to find Tile-Pos file for TO-1, your resources may be outdated"
    ));
}

#[test]
fn parses_hot_update_version_json() {
    let version = parse_hot_update_version(
            r#"{"activity":{"name":"直到大地变成一颗酸橙","time":1},"last_updated":"2026-08-01 16:21:34.000"}"#,
        )
        .unwrap();
    assert_eq!(version.activity_name, "直到大地变成一颗酸橙");
    assert_eq!(version.last_updated, "2026-08-01 16:21:34.000");
    assert!(version.summary().contains("直到大地变成一颗酸橙"));
}

#[test]
fn outdated_resource_failure_appends_actionable_hint() {
    let mut app = App::new();
    app.active_label = "自动战斗".to_string();
    app.active_log_scope = LogScope::Copilot;
    app.phase = TaskPhase::Running;
    app.push_log(
        LogLevel::Error,
        "Error: Failed to find Tile-Pos file for TO-1, your resources may be outdated",
    );
    assert!(app.saw_outdated_resource_error);
    assert!(!app.saw_ocr_to_t0_error);

    app.on_exited(Some(1), false);

    let logs = &app.log_buffer(LogScope::Copilot).lines;
    assert!(logs.iter().any(|log| log.text == OUTDATED_RESOURCE_HINT));
    assert!(logs.iter().all(|log| log.text != OCR_TO_T0_HINT));
    assert!(!app.saw_outdated_resource_error);
}

#[test]
fn ocr_to_as_t0_failure_appends_core_upgrade_hint() {
    let mut app = App::new();
    app.active_label = "自动战斗".to_string();
    app.active_log_scope = LogScope::Copilot;
    app.phase = TaskPhase::Running;
    app.push_log(
        LogLevel::Error,
        "Error: Failed to find Tile-Pos file for T0-1, your resources may be outdated",
    );
    assert!(app.saw_ocr_to_t0_error);
    assert!(!app.saw_outdated_resource_error);

    app.on_exited(Some(1), false);

    let logs = &app.log_buffer(LogScope::Copilot).lines;
    assert!(logs.iter().any(|log| log.text == OCR_TO_T0_HINT));
    assert!(logs.iter().all(|log| log.text != OUTDATED_RESOURCE_HINT));
    assert!(!app.saw_ocr_to_t0_error);
}

#[test]
fn resource_update_success_warns_when_version_unchanged() {
    let _guard = hot_update_env_lock();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("maatui-hot-update-{unique}"));
    let version_dir = root.join("resource");
    fs::create_dir_all(&version_dir).unwrap();
    let version_path = version_dir.join("version.json");
    fs::write(
        &version_path,
        r#"{"activity":{"name":"旧活动"},"last_updated":"2026-07-21 08:04:12.000"}"#,
    )
    .unwrap();

    let previous = std::env::var_os("MAA_HOT_UPDATE_DIR");
    unsafe {
        std::env::set_var("MAA_HOT_UPDATE_DIR", &root);
    }

    let mut app = App::new();
    app.active_label = "资源热更新".to_string();
    app.active_log_scope = LogScope::Update;
    app.pending_resource_check = true;
    app.resource_version_before = read_hot_update_version();
    app.on_exited(Some(0), false);

    assert!(
        app.log_buffer(LogScope::Update).lines.iter().any(|log| {
            log.level == LogLevel::Warn && log.text.contains("version.json 未变化")
        })
    );
    assert!(app.status_text.contains("未变化"));

    match previous {
        Some(value) => unsafe { std::env::set_var("MAA_HOT_UPDATE_DIR", value) },
        None => unsafe { std::env::remove_var("MAA_HOT_UPDATE_DIR") },
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resource_update_success_reports_new_version() {
    let _guard = hot_update_env_lock();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("maatui-hot-update-new-{unique}"));
    let version_dir = root.join("resource");
    fs::create_dir_all(&version_dir).unwrap();
    fs::write(
        version_dir.join("version.json"),
        r#"{"activity":{"name":"直到大地变成一颗酸橙"},"last_updated":"2026-08-01 16:21:34.000"}"#,
    )
    .unwrap();

    let previous = std::env::var_os("MAA_HOT_UPDATE_DIR");
    unsafe {
        std::env::set_var("MAA_HOT_UPDATE_DIR", &root);
    }

    let mut app = App::new();
    app.active_label = "资源热更新".to_string();
    app.active_log_scope = LogScope::Update;
    app.pending_resource_check = true;
    app.resource_version_before = Some(HotUpdateVersion {
        activity_name: "旧活动".to_string(),
        last_updated: "2026-07-21 08:04:12.000".to_string(),
    });
    app.on_exited(Some(0), false);

    assert!(app.log_buffer(LogScope::Update).lines.iter().any(|log| {
        log.level == LogLevel::System && log.text.contains("直到大地变成一颗酸橙")
    }));
    assert!(app.status_text.contains("直到大地变成一颗酸橙"));

    match previous {
        Some(value) => unsafe { std::env::set_var("MAA_HOT_UPDATE_DIR", value) },
        None => unsafe { std::env::remove_var("MAA_HOT_UPDATE_DIR") },
    }
    fs::remove_dir_all(root).unwrap();
}
