//! 应用状态、分层菜单、配置编辑与任务状态迁移。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

#[cfg(test)]
use crate::config::TaskSummary;
use crate::config::{DailyConfig, FieldValue};
use crate::copilot::{
    BatchTask, CopilotCache, CopilotDetail, CopilotRunOptions, ImportKind, ImportReport,
    import_source, remove_batch_task, write_batch_task,
};
use crate::copilot_run::{BatchPosition, CopilotBatchState};
use crate::runner::{LogLevel, RunnerEvent, RunningTask, TaskCommand, TaskKind};
use crate::shortcuts::{Action, ShortcutContext, action_for};
use crate::stage::{StageCatalog, StageRefreshEvent};
use crate::storage::maa_hot_update_dir;
use crate::tile_alias;

const MAX_LOG_LINES: usize = 3000;
const OUTDATED_RESOURCE_HINT: &str = "地图资源可能过旧或关卡码别名缺失：请先热更新资源；MaaTUI 会在启动自动战斗时尝试为 overview 中的关卡码生成 Tile-Pos 别名。若仍失败，检查 `maa dir hot-update` 的 git HEAD 是否落后 remote";
const OCR_TO_T0_HINT: &str = "关卡码疑似 OCR 将 TO 识别为 T0：请到「更新管理」执行「更新 MaaCore 与基础资源」（需 MaaCore ≥ 6.16）；仅热更新资源通常不够";
pub const TASK_TYPES: [&str; 8] = [
    "StartUp",
    "Recruit",
    "Fight",
    "Infrast",
    "Mall",
    "Award",
    "CloseDown",
    "Copilot",
];

mod state;

pub use state::*;
use state::{CopilotImportEvent, DailyRunState, SelectBehavior, option_label};

impl App {
    pub fn new() -> Self {
        let (config, config_error) = match DailyConfig::load_default() {
            Ok(config) => (Some(config), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let (copilot_cache, copilot_error) = match CopilotCache::load_default() {
            Ok(cache) => (Some(cache), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let copilot = copilot_cache
            .as_ref()
            .map(|cache| cache.settings().clone())
            .unwrap_or_default();
        let migrated_single_reset = copilot_cache
            .as_ref()
            .is_some_and(CopilotCache::migrated_single_reset);
        let current_single_supported_modes = copilot_cache
            .as_ref()
            .and_then(|cache| cache.current_single_supported_modes().ok());
        let (copilot_import_tx, copilot_import_rx) = mpsc::channel();
        let (stage_refresh_tx, stage_refresh_rx) = mpsc::channel();
        let stage_catalog = StageCatalog::load_cached();
        let stage_refresh_error = StageCatalog::spawn_refresh(stage_refresh_tx.clone()).err();
        let stage_refreshing = stage_refresh_error.is_none();
        let mut app = Self {
            screen: Screen::Main,
            main_idx: 0,
            daily_idx: 0,
            config_idx: 0,
            add_task_idx: 0,
            section_idx: 0,
            field_idx: 0,
            variant_idx: 0,
            copilot_idx: 0,
            copilot_settings_idx: 0,
            copilot_section_idx: 0,
            update_idx: 0,
            phase: TaskPhase::Idle,
            menu_logs: MenuLogs::default(),
            active_log_scope: LogScope::Daily,
            status_text: "就绪".to_string(),
            should_quit: false,
            started_at: None,
            active_label: String::new(),
            last_failed: false,
            config,
            config_error,
            copilot,
            copilot_cache,
            copilot_error,
            current_single_supported_modes,
            copilot_importing: false,
            copilot_import_progress: None,
            stage_catalog,
            input: None,
            select: None,
            confirm: None,
            copilot_detail: None,
            shortcut_help_open: false,
            shortcut_help_scroll: 0,
            run_progress: None,
            temp_task_file: None,
            task: None,
            daily_run: None,
            pending_resource_check: false,
            resource_version_before: None,
            saw_outdated_resource_error: false,
            saw_ocr_to_t0_error: false,
            copilot_batch: None,
            copilot_import_tx,
            copilot_import_rx,
            stage_refresh_tx,
            stage_refresh_rx,
            stage_refreshing,
            animation_started: Instant::now(),
        };
        if migrated_single_reset {
            let message =
                "缓存已升级：旧单作业列表已清空，请重新搜索当前单作业；作业集列表保持不变";
            app.push_log_to(LogScope::Copilot, LogLevel::Warn, message);
        }
        if let Some(error) = stage_refresh_error {
            let message = format!("启动活动关卡刷新线程失败: {error}");
            if !migrated_single_reset {
                app.status_text = message.clone();
            }
            app.push_log_to(LogScope::Update, LogLevel::Warn, message);
        }
        app
    }

    pub fn selected_main(&self) -> MainMenuItem {
        MainMenuItem::ALL[self.main_idx.min(MainMenuItem::ALL.len() - 1)]
    }

    pub fn editor_section(&self) -> EditorSection {
        EditorSection::ALL[self.section_idx.min(EditorSection::ALL.len() - 1)]
    }

    pub fn copilot_section(&self) -> CopilotSection {
        CopilotSection::ALL[self.copilot_section_idx.min(CopilotSection::ALL.len() - 1)]
    }

    pub fn shortcut_context(&self) -> ShortcutContext {
        ShortcutContext::from_app(
            self.phase,
            self.screen,
            self.editor_section(),
            self.copilot_section(),
        )
    }

    pub fn copilot_visible_indices(&self) -> Vec<usize> {
        if self.copilot_section() != CopilotSection::Sets {
            return Vec::new();
        }
        self.copilot_cache
            .as_ref()
            .map_or_else(Vec::new, |cache| (0..cache.entries_len()).collect())
    }

    fn selected_copilot_index(&self) -> Option<usize> {
        self.copilot_visible_indices()
            .get(self.copilot_idx)
            .copied()
    }

    fn clamp_copilot_selection(&mut self) {
        let len = self.copilot_visible_indices().len();
        self.copilot_idx = self.copilot_idx.min(len.saturating_sub(1));
    }

    pub fn current_run_progress(&self) -> Option<&RunProgress> {
        self.run_progress.as_ref()
    }

    pub fn copilot_batch_progress(&self) -> Option<(usize, usize, String)> {
        let batch = self.copilot_batch.as_ref()?;
        match batch.position()? {
            BatchPosition::Finishing { total } => {
                Some((total, total, "全部作业已完成，正在收尾".to_string()))
            }
            BatchPosition::Running {
                current,
                total,
                index,
            } => {
                let name = self
                    .copilot_cache
                    .as_ref()
                    .and_then(|cache| cache.entry(index))
                    .map(|entry| format!("{} · {}", entry.stage_name, entry.display_name()))
                    .unwrap_or_else(|| format!("作业 {}", index + 1));
                Some((current, total, name))
            }
        }
    }

    pub fn visible_log_scope(&self) -> LogScope {
        if self.phase != TaskPhase::Idle {
            return self.active_log_scope;
        }
        match self.screen {
            Screen::Daily
            | Screen::Config
            | Screen::AddTask
            | Screen::TaskEdit
            | Screen::VariantList
            | Screen::VariantEdit => LogScope::Daily,
            Screen::Copilot => LogScope::Copilot,
            Screen::Update => LogScope::Update,
            Screen::Main => self.active_log_scope,
        }
    }

    pub fn log_buffer(&self, scope: LogScope) -> &LogBuffer {
        self.menu_logs.get(scope)
    }

    pub fn log_buffer_mut(&mut self, scope: LogScope) -> &mut LogBuffer {
        self.menu_logs.get_mut(scope)
    }

    pub fn push_log_to(&mut self, scope: LogScope, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        if scope == LogScope::Copilot
            && self.phase != TaskPhase::Idle
            && self.active_log_scope == LogScope::Copilot
        {
            if looks_like_ocr_to_as_t0_error(&text) {
                self.saw_ocr_to_t0_error = true;
            } else if looks_like_outdated_resource_error(&text) {
                self.saw_outdated_resource_error = true;
            }
        }
        let buffer = self.log_buffer_mut(scope);
        buffer.lines.push_back(LogLine { level, text });
        while buffer.lines.len() > MAX_LOG_LINES {
            buffer.lines.pop_front();
        }
        if buffer.auto_scroll {
            buffer.scroll = buffer.lines.len().saturating_sub(1) as u16;
        }
    }

    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        let scope = self.visible_log_scope();
        self.push_log_to(scope, level, text);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.shortcut_help_open {
            self.handle_shortcut_help_key(key);
            return;
        }
        if self.copilot_detail.is_some() {
            self.handle_copilot_detail_key(key);
            return;
        }
        if self.input.is_some() {
            self.handle_input_key(key);
            return;
        }
        if self.select.is_some() {
            self.handle_select_key(key);
            return;
        }
        if self.confirm.is_some() {
            self.handle_confirm_key(key);
            return;
        }

        if key.code == KeyCode::Char('?') {
            self.shortcut_help_open = true;
            self.shortcut_help_scroll = 0;
            return;
        }

        if self.phase != TaskPhase::Idle {
            match action_for(self.shortcut_context(), key) {
                Some(Action::Stop) => self.request_stop(),
                Some(Action::QuitRunning) => self.request_quit(),
                Some(Action::LogPageUp) => self.scroll_logs_up(10),
                Some(Action::LogPageDown) => self.scroll_logs_down(10),
                Some(Action::LogHome) => {
                    let scope = self.visible_log_scope();
                    let buffer = self.log_buffer_mut(scope);
                    buffer.auto_scroll = false;
                    buffer.scroll = 0;
                }
                Some(Action::LogEnd) => {
                    let scope = self.visible_log_scope();
                    let buffer = self.log_buffer_mut(scope);
                    buffer.auto_scroll = true;
                    buffer.scroll = u16::MAX;
                }
                _ => {}
            }
            return;
        }

        match self.screen {
            Screen::Main => self.handle_main_key(key),
            Screen::Daily => self.handle_daily_key(key),
            Screen::Config => self.handle_config_key(key),
            Screen::AddTask => self.handle_add_task_key(key),
            Screen::TaskEdit => self.handle_task_edit_key(key),
            Screen::VariantList => self.handle_variant_list_key(key),
            Screen::VariantEdit => self.handle_variant_edit_key(key),
            Screen::Copilot => self.handle_copilot_key(key),
            Screen::Update => self.handle_update_key(key),
        }
    }

    fn handle_shortcut_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                self.shortcut_help_open = false;
                self.shortcut_help_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_add(10);
            }
            KeyCode::Home => self.shortcut_help_scroll = 0,
            KeyCode::End => self.shortcut_help_scroll = u16::MAX,
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.shortcut_help_open {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown => {
                    self.shortcut_help_scroll = self.shortcut_help_scroll.saturating_add(3)
                }
                _ => {}
            }
            return;
        }
        if self.copilot_detail.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.scroll_copilot_detail_up(3),
                MouseEventKind::ScrollDown => self.scroll_copilot_detail_down(3),
                _ => {}
            }
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_logs_up(3),
            MouseEventKind::ScrollDown => self.scroll_logs_down(3),
            _ => {}
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        match action_for(ShortcutContext::Main, key) {
            Some(Action::Back) => self.request_quit(),
            Some(Action::Previous) => self.main_idx = self.main_idx.saturating_sub(1),
            Some(Action::Next) => {
                self.main_idx = (self.main_idx + 1).min(MainMenuItem::ALL.len() - 1);
            }
            Some(Action::Activate) => match self.selected_main() {
                MainMenuItem::Daily => {
                    self.daily_idx = 0;
                    self.screen = Screen::Daily;
                }
                MainMenuItem::Copilot => {
                    self.copilot_section_idx = 0;
                    self.screen = Screen::Copilot;
                }
                MainMenuItem::Update => {
                    self.update_idx = 0;
                    self.screen = Screen::Update;
                }
                MainMenuItem::Quit => self.request_quit(),
            },
            _ => {}
        }
    }

    fn handle_daily_key(&mut self, key: KeyEvent) {
        match action_for(ShortcutContext::Daily, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => {
                self.daily_idx = self.daily_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                self.daily_idx = (self.daily_idx + 1).min(1);
            }
            Some(Action::RunDaily) => {
                self.start_command(TaskCommand::daily());
            }
            Some(Action::OpenConfig) => {
                self.reload_config();
                self.screen = Screen::Config;
            }
            Some(Action::Activate) => match self.daily_idx {
                0 => {
                    self.start_command(TaskCommand::daily());
                }
                1 => {
                    self.reload_config();
                    self.screen = Screen::Config;
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_config_key(&mut self, key: KeyEvent) {
        let len = self.config.as_ref().map_or(0, DailyConfig::len);
        match action_for(ShortcutContext::Config, key) {
            Some(Action::Back) => self.screen = Screen::Daily,
            Some(Action::MoveUp) if len > 0 && self.config_idx > 0 => {
                let from = self.config_idx;
                if self.apply_config(|config| config.move_task(from, from - 1)) {
                    self.config_idx -= 1;
                }
            }
            Some(Action::MoveDown) if len > 0 && self.config_idx + 1 < len => {
                let from = self.config_idx;
                if self.apply_config(|config| config.move_task(from, from + 1)) {
                    self.config_idx += 1;
                }
            }
            Some(Action::Previous) => {
                self.config_idx = self.config_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                if len > 0 {
                    self.config_idx = (self.config_idx + 1).min(len - 1);
                }
            }
            Some(Action::Toggle) => {
                if len > 0 {
                    let index = self.config_idx;
                    self.apply_config(|config| config.toggle_task(index).map(|_| ()));
                }
            }
            Some(Action::Add) => {
                self.add_task_idx = 0;
                self.screen = Screen::AddTask;
            }
            Some(Action::Delete) => {
                if len > 0 {
                    let name = self
                        .config
                        .as_ref()
                        .and_then(|config| config.task_name(self.config_idx))
                        .unwrap_or_else(|| "当前任务".to_string());
                    self.confirm = Some(ConfirmDialog {
                        message: format!("确认删除“{name}”？此操作会立即保存。"),
                        target: ConfirmTarget::DeleteTask(self.config_idx),
                    });
                }
            }
            Some(Action::Edit) => {
                if len > 0 {
                    self.section_idx = 0;
                    self.field_idx = 0;
                    self.screen = Screen::TaskEdit;
                }
            }
            Some(Action::Reload) => self.reload_config(),
            Some(Action::RunDailySingle) => {
                if len == 0 {
                    return;
                }
                let index = self.config_idx;
                let Some(config) = self.config.as_ref() else {
                    return;
                };
                let name = config
                    .task_name(index)
                    .unwrap_or_else(|| format!("任务 {}", index + 1));
                match config.write_single_task_file(index) {
                    Ok((path, basename)) => {
                        let command = TaskCommand::daily_single(index, &name, &basename, path);
                        // 跳转到每日任务执行页显示日志。
                        self.screen = Screen::Daily;
                        self.start_command(command);
                    }
                    Err(error) => {
                        self.status_error(format!("生成单任务配置失败: {error}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_add_task_key(&mut self, key: KeyEvent) {
        match action_for(ShortcutContext::AddTask, key) {
            Some(Action::Back) => self.screen = Screen::Config,
            Some(Action::Previous) => {
                self.add_task_idx = self.add_task_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                self.add_task_idx = (self.add_task_idx + 1).min(TASK_TYPES.len() - 1);
            }
            Some(Action::Activate) => {
                let task_type = TASK_TYPES[self.add_task_idx];
                let mut new_index = None;
                self.apply_config(|config| {
                    new_index = Some(config.add_task(task_type)?);
                    Ok(())
                });
                if let Some(index) = new_index {
                    self.config_idx = index;
                    self.screen = Screen::TaskEdit;
                    self.section_idx = 0;
                    self.field_idx = 0;
                }
            }
            _ => {}
        }
    }

    fn handle_task_edit_key(&mut self, key: KeyEvent) {
        let context = if self.editor_section() == EditorSection::Variants {
            ShortcutContext::TaskVariants
        } else {
            ShortcutContext::TaskEdit
        };
        match action_for(context, key) {
            Some(Action::Back) => {
                self.field_idx = 0;
                self.screen = Screen::Config;
            }
            Some(Action::PreviousEditorSection) => {
                self.section_idx = self.section_idx.saturating_sub(1);
                self.field_idx = 0;
            }
            Some(Action::NextEditorSection) => {
                self.section_idx = (self.section_idx + 1).min(EditorSection::ALL.len() - 1);
                self.field_idx = 0;
            }
            Some(Action::Edit) if self.editor_section() == EditorSection::Variants => {
                self.variant_idx = 0;
                self.screen = Screen::VariantList;
            }
            Some(Action::Previous) => self.field_idx = self.field_idx.saturating_sub(1),
            Some(Action::Next) => {
                let len = self.current_task_fields().len();
                if len > 0 {
                    self.field_idx = (self.field_idx + 1).min(len - 1);
                }
            }
            Some(Action::CreateVariant) => self.create_stage_variant(),
            Some(Action::Edit) => self.edit_current_task_field(),
            _ => {}
        }
    }

    fn handle_variant_list_key(&mut self, key: KeyEvent) {
        let count = self
            .config
            .as_ref()
            .map_or(0, |config| config.variant_count(self.config_idx));
        match action_for(ShortcutContext::VariantList, key) {
            Some(Action::Back) => self.screen = Screen::TaskEdit,
            Some(Action::MoveUp) if count > 0 && self.variant_idx > 0 => {
                let task = self.config_idx;
                let from = self.variant_idx;
                if self.apply_config(|config| config.move_variant(task, from, from - 1)) {
                    self.variant_idx -= 1;
                }
            }
            Some(Action::MoveDown) if count > 0 && self.variant_idx + 1 < count => {
                let task = self.config_idx;
                let from = self.variant_idx;
                if self.apply_config(|config| config.move_variant(task, from, from + 1)) {
                    self.variant_idx += 1;
                }
            }
            Some(Action::Previous) => {
                self.variant_idx = self.variant_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                if count > 0 {
                    self.variant_idx = (self.variant_idx + 1).min(count - 1);
                }
            }
            Some(Action::Add) => {
                let task = self.config_idx;
                let mut new_index = None;
                self.apply_config(|config| {
                    new_index = Some(config.add_variant(task)?);
                    Ok(())
                });
                if let Some(index) = new_index {
                    self.variant_idx = index;
                }
            }
            Some(Action::Delete) => {
                if count > 0 {
                    self.confirm = Some(ConfirmDialog {
                        message: "确认删除当前变体？此操作会立即保存。".to_string(),
                        target: ConfirmTarget::DeleteVariant {
                            task: self.config_idx,
                            variant: self.variant_idx,
                        },
                    });
                }
            }
            Some(Action::Edit) if count > 0 => {
                self.field_idx = 0;
                self.screen = Screen::VariantEdit;
            }
            _ => {}
        }
    }

    fn handle_variant_edit_key(&mut self, key: KeyEvent) {
        let fields = variant_fields();
        match action_for(ShortcutContext::VariantEdit, key) {
            Some(Action::Back) => self.screen = Screen::VariantList,
            Some(Action::Previous) => self.field_idx = self.field_idx.saturating_sub(1),
            Some(Action::Next) => {
                self.field_idx = (self.field_idx + 1).min(fields.len() - 1);
            }
            Some(Action::Edit) => {
                let field = fields[self.field_idx].clone();
                let current = self
                    .variant_field_value(&field)
                    .unwrap_or_else(|| field.default.clone());
                let condition_type = self.config.as_ref().and_then(|config| {
                    config.variant_condition_value(self.config_idx, self.variant_idx, "type")
                });
                if field.scope == FieldScope::VariantCondition
                    && condition_type
                        .as_ref()
                        .is_some_and(|value| !is_editable_condition_type(value))
                {
                    self.status_text =
                        "该组合或未知条件暂不支持可视化编辑，已保留原配置".to_string();
                    return;
                }
                let target = InputTarget::VariantField {
                    task: self.config_idx,
                    variant: self.variant_idx,
                    field: field.clone(),
                };
                self.open_field_editor(
                    format!("编辑 {}", field.label),
                    current,
                    field.editor.clone(),
                    target,
                );
            }
            _ => {}
        }
    }

    fn handle_copilot_key(&mut self, key: KeyEvent) {
        let context = match self.copilot_section() {
            CopilotSection::Singles => ShortcutContext::CopilotSingles,
            CopilotSection::Sets => ShortcutContext::CopilotSets,
            CopilotSection::Settings => ShortcutContext::CopilotSettings,
        };
        match action_for(context, key) {
            Some(Action::NextCopilotTab) => {
                self.copilot_section_idx =
                    (self.copilot_section_idx + 1) % CopilotSection::ALL.len();
                self.clamp_copilot_selection();
                return;
            }
            Some(Action::PreviousCopilotTab) => {
                self.copilot_section_idx = self.copilot_section_idx.saturating_sub(1);
                self.clamp_copilot_selection();
                return;
            }
            _ => {}
        }

        match self.copilot_section() {
            CopilotSection::Singles => self.handle_single_copilot_key(key),
            CopilotSection::Sets => self.handle_copilot_list_key(key),
            CopilotSection::Settings => self.handle_copilot_settings_key(key),
        }
    }

    fn handle_single_copilot_key(&mut self, key: KeyEvent) {
        match action_for(ShortcutContext::CopilotSingles, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::SearchSingle) if self.copilot_importing => {
                self.status_text = "已有作业正在导入，请稍候".to_string();
            }
            Some(Action::SearchSingle) => self.open_input(
                "搜索当前单作业：代码 / URI / 本地 JSON".to_string(),
                String::new(),
                InputTarget::CopilotAdd {
                    kind: ImportKind::Single,
                    destination: ImportDestination::CurrentSingle,
                },
            ),
            Some(Action::ToggleDifficulty) => self.toggle_current_single_difficulty(),
            Some(Action::ShowDetail) => self.open_current_single_detail(),
            Some(Action::RunSingle) => self.start_current_single_copilot(),
            _ => {}
        }
    }

    fn toggle_current_single_difficulty(&mut self) {
        let result = self
            .copilot_cache
            .as_mut()
            .context("作业缓存未加载")
            .and_then(CopilotCache::toggle_current_single_raid);
        match result {
            Ok(_) => self.last_failed = false,
            Err(error) => self.status_error(error.to_string()),
        }
    }

    fn handle_copilot_list_key(&mut self, key: KeyEvent) {
        let visible = self.copilot_visible_indices();
        let len = visible.len();
        let enabled_count = self
            .copilot_cache
            .as_ref()
            .map_or(0, |cache| cache.enabled_count());
        match action_for(ShortcutContext::CopilotSets, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::MoveUp) if len > 0 && self.copilot_idx > 0 => {
                let from = visible[self.copilot_idx];
                let to = visible[self.copilot_idx - 1];
                if self.apply_copilot_cache(|cache| cache.swap_entries(from, to)) {
                    self.copilot_idx -= 1;
                }
            }
            Some(Action::MoveDown) if len > 0 && self.copilot_idx + 1 < len => {
                let from = visible[self.copilot_idx];
                let to = visible[self.copilot_idx + 1];
                if self.apply_copilot_cache(|cache| cache.swap_entries(from, to)) {
                    self.copilot_idx += 1;
                }
            }
            Some(Action::Previous) => self.copilot_idx = self.copilot_idx.saturating_sub(1),
            Some(Action::Next) => {
                if len > 0 {
                    self.copilot_idx = (self.copilot_idx + 1).min(len - 1);
                }
            }
            Some(Action::Toggle) => {
                if len > 0 {
                    let index = visible[self.copilot_idx];
                    self.apply_copilot_cache(|cache| cache.toggle(index).map(|_| ()));
                }
            }
            Some(Action::Add) if self.copilot_importing => {
                self.status_text = "已有作业正在导入，请稍候".to_string();
            }
            Some(Action::Add) => {
                self.open_input(
                    "添加到作业集".to_string(),
                    String::new(),
                    InputTarget::CopilotAdd {
                        kind: ImportKind::Set,
                        destination: ImportDestination::Batch,
                    },
                );
            }
            Some(Action::ToggleAll) => {
                if len > 0 {
                    let enable = enabled_count == 0;
                    self.apply_copilot_cache(|cache| cache.set_all_enabled(enable));
                }
            }
            Some(Action::Clear) => {
                if len > 0 {
                    self.confirm = Some(ConfirmDialog {
                        message: format!("确认清空全部 {len} 个批量作业？此操作会立即保存。"),
                        target: ConfirmTarget::ClearCopilotEntries,
                    });
                }
            }
            Some(Action::ShowDetail) => {
                if len > 0 {
                    self.open_selected_copilot_detail();
                }
            }
            Some(Action::Delete) => {
                if len > 0 {
                    let index = visible[self.copilot_idx];
                    let name = self
                        .copilot_cache
                        .as_ref()
                        .and_then(|cache| cache.entry(index))
                        .map(|entry| entry.display_name())
                        .unwrap_or_else(|| "当前作业".to_string());
                    self.confirm = Some(ConfirmDialog {
                        message: format!("确认删除“{name}”？此操作会立即保存。"),
                        target: ConfirmTarget::DeleteCopilot(index),
                    });
                }
            }
            Some(Action::RunSelected) => {
                if len > 0 {
                    self.start_selected_copilot();
                }
            }
            Some(Action::RunBatch) => {
                if len == 0 {
                    self.status_text = "作业集为空，请先添加作业".to_string();
                } else if enabled_count == 0 {
                    self.status_text = "没有启用的作业".to_string();
                } else {
                    self.start_copilot_batch();
                }
            }
            _ => {}
        }
    }

    fn save_copilot_settings(&mut self) {
        let settings = self.copilot.clone();
        let result = self
            .copilot_cache
            .as_mut()
            .context("作业缓存未加载")
            .and_then(|cache| cache.set_settings(settings));
        if let Err(error) = result {
            if let Some(cache) = self.copilot_cache.as_ref() {
                self.copilot = cache.settings().clone();
            }
            self.status_error(format!("保存自动战斗设置失败: {error}"));
        } else {
            self.last_failed = false;
        }
    }

    fn handle_copilot_settings_key(&mut self, key: KeyEvent) {
        const ROWS: usize = 8;
        match action_for(ShortcutContext::CopilotSettings, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => {
                self.copilot_settings_idx = self.copilot_settings_idx.saturating_sub(1)
            }
            Some(Action::Next) => {
                self.copilot_settings_idx = (self.copilot_settings_idx + 1).min(ROWS - 1);
            }
            Some(Action::RunBatch) => {
                let enabled_count = self
                    .copilot_cache
                    .as_ref()
                    .map_or(0, |cache| cache.enabled_count());
                if enabled_count == 0 {
                    self.status_text = "没有启用的作业".to_string();
                } else {
                    self.start_copilot_batch();
                }
            }
            Some(Action::Edit) => match self.copilot_settings_idx {
                0 => {
                    self.copilot.formation = !self.copilot.formation;
                    self.save_copilot_settings();
                }
                1 => self.open_select(
                    "编队编号".to_string(),
                    FieldValue::Integer(self.copilot.formation_index),
                    copilot_formation_options(),
                    false,
                    InputTarget::CopilotNumber(CopilotNumberField::FormationIndex),
                ),
                2 => {
                    self.copilot.use_sanity_potion = !self.copilot.use_sanity_potion;
                    self.save_copilot_settings();
                }
                3 => {
                    self.copilot.add_trust = !self.copilot.add_trust;
                    self.save_copilot_settings();
                }
                4 => {
                    self.copilot.ignore_requirements = !self.copilot.ignore_requirements;
                    self.save_copilot_settings();
                }
                5 => self.open_select(
                    "助战模式".to_string(),
                    FieldValue::Integer(self.copilot.support_unit_usage),
                    support_usage_options(),
                    false,
                    InputTarget::CopilotNumber(CopilotNumberField::SupportUsage),
                ),
                6 => self.open_input(
                    "指定助战干员".to_string(),
                    self.copilot.support_unit_name.clone(),
                    InputTarget::CopilotText(CopilotTextField::SupportName),
                ),
                7 => self.open_input(
                    "循环次数（仅单作业，大于 0）".to_string(),
                    self.copilot.loop_times.to_string(),
                    InputTarget::CopilotNumber(CopilotNumberField::LoopTimes),
                ),
                _ => {}
            },
            _ => {}
        }
    }

    fn open_current_single_detail(&mut self) {
        let result = self
            .copilot_cache
            .as_ref()
            .context("作业缓存未加载")
            .and_then(CopilotCache::current_single_detail);
        match result {
            Ok(CopilotDetail { title, lines }) => {
                self.copilot_detail = Some(CopilotDetailDialog {
                    title: format!("单作业详情 · {title}"),
                    lines,
                    scroll: 0,
                });
            }
            Err(error) => self.status_error(error.to_string()),
        }
    }

    fn open_selected_copilot_detail(&mut self) {
        let Some(index) = self.selected_copilot_index() else {
            return;
        };
        let result = self
            .copilot_cache
            .as_ref()
            .context("作业列表未加载")
            .and_then(|cache| cache.detail(index));
        match result {
            Ok(CopilotDetail { title, lines }) => {
                debug_assert_eq!(self.copilot_section(), CopilotSection::Sets);
                self.copilot_detail = Some(CopilotDetailDialog {
                    title: format!("作业集条目详情 · {title}"),
                    lines,
                    scroll: 0,
                });
            }
            Err(error) => self.status_error(error.to_string()),
        }
    }

    fn handle_copilot_detail_key(&mut self, key: KeyEvent) {
        let Some(detail) = self.copilot_detail.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.copilot_detail = None,
            KeyCode::Up | KeyCode::Char('k') => detail.scroll = detail.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => detail.scroll = detail.scroll.saturating_add(1),
            KeyCode::PageUp => detail.scroll = detail.scroll.saturating_sub(10),
            KeyCode::PageDown => detail.scroll = detail.scroll.saturating_add(10),
            KeyCode::Home => detail.scroll = 0,
            KeyCode::End => detail.scroll = u16::MAX,
            _ => {}
        }
    }

    fn scroll_copilot_detail_up(&mut self, amount: u16) {
        if let Some(detail) = self.copilot_detail.as_mut() {
            detail.scroll = detail.scroll.saturating_sub(amount);
        }
    }

    fn scroll_copilot_detail_down(&mut self, amount: u16) {
        if let Some(detail) = self.copilot_detail.as_mut() {
            detail.scroll = detail.scroll.saturating_add(amount);
        }
    }

    fn handle_update_key(&mut self, key: KeyEvent) {
        const ROWS: usize = 2;
        match action_for(ShortcutContext::Update, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => self.update_idx = self.update_idx.saturating_sub(1),
            Some(Action::Next) => {
                self.update_idx = (self.update_idx + 1).min(ROWS - 1);
            }
            Some(Action::Activate) => {
                let (message, command) = match self.update_idx {
                    0 => (
                        "确认执行 `maa hot-update --batch -v`？\n仅更新活动与导航资源（MaaResource），不更新 MaaCore 和基础资源。",
                        TaskCommand::resource_update(),
                    ),
                    1 => (
                        "确认执行 `maa update --batch -v`？\n将按已配置频道更新 MaaCore 与随包基础资源，适合修复 OCR/兼容性问题。",
                        TaskCommand::core_update(),
                    ),
                    _ => return,
                };
                self.confirm = Some(ConfirmDialog {
                    message: message.to_string(),
                    target: ConfirmTarget::RunCommand(command),
                });
            }
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => self.commit_input(),
            KeyCode::Left
                if self
                    .input
                    .as_ref()
                    .is_some_and(InputDialog::allows_import_kind_switch) =>
            {
                if let Some(input) = self.input.as_mut() {
                    input.select_batch_import_kind(ImportKind::Set);
                }
            }
            KeyCode::Right
                if self
                    .input
                    .as_ref()
                    .is_some_and(InputDialog::allows_import_kind_switch) =>
            {
                if let Some(input) = self.input.as_mut() {
                    input.select_batch_import_kind(ImportKind::Single);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(input) = self.input.as_mut() {
                    input.value.clear();
                }
            }
            KeyCode::Backspace => {
                if let Some(input) = self.input.as_mut() {
                    input.value.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Some(input) = self.input.as_mut() {
                    input.value.push(character);
                }
            }
            _ => {}
        }
    }

    fn handle_select_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.select = None,
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(select) = self.select.as_mut() {
                    select.selected = select.selected.saturating_sub(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(select) = self.select.as_mut()
                    && !select.options.is_empty()
                {
                    select.selected = (select.selected + 1).min(select.options.len() - 1);
                }
            }
            KeyCode::Char(' ') if self.select.as_ref().is_some_and(|select| select.multi) => {
                if let Some(select) = self.select.as_mut()
                    && let Some(option) = select.options.get(select.selected)
                {
                    if let Some(index) = select
                        .selected_values
                        .iter()
                        .position(|value| value == &option.value)
                    {
                        select.selected_values.remove(index);
                    } else {
                        select.selected_values.push(option.value.clone());
                    }
                }
            }
            KeyCode::Char('r')
                if self
                    .select
                    .as_ref()
                    .is_some_and(|select| select.refreshable) =>
            {
                self.refresh_stage_catalog();
            }
            KeyCode::Char('c')
                if self
                    .select
                    .as_ref()
                    .is_some_and(|select| select.allow_custom) =>
            {
                if let Some(select) = self.select.take() {
                    self.open_input(
                        format!("{} · 自定义", select.title),
                        select.current.display(),
                        select.target,
                    );
                }
            }
            KeyCode::Enter => {
                let Some(select) = self.select.take() else {
                    return;
                };
                let value = if select.multi {
                    Self::selected_multi_value(&select)
                } else {
                    let Some(option) = select.options.get(select.selected) else {
                        return;
                    };
                    option.value.clone()
                };
                if let Err(error) = self.commit_target_value(select.target, value) {
                    self.status_error(error.to_string());
                }
            }
            _ => {}
        }
    }

    fn selected_multi_value(select: &SelectDialog) -> FieldValue {
        match select.current {
            FieldValue::IntegerArray(_) => FieldValue::IntegerArray(
                select
                    .selected_values
                    .iter()
                    .filter_map(|value| match value {
                        FieldValue::Integer(value) => Some(*value),
                        _ => None,
                    })
                    .collect(),
            ),
            FieldValue::StringArray(_) => FieldValue::StringArray(
                select
                    .selected_values
                    .iter()
                    .filter_map(|value| match value {
                        FieldValue::String(value) => Some(value.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => select.current.clone(),
        }
    }

    fn commit_target_value(
        &mut self,
        target: InputTarget,
        value: FieldValue,
    ) -> anyhow::Result<()> {
        match target {
            InputTarget::TaskField { task, field } => {
                self.set_task_field_result(task, &field, value)
            }
            InputTarget::VariantField {
                task,
                variant,
                field,
            } => self.set_variant_field_result(task, variant, &field, value),
            InputTarget::CopilotNumber(field) => {
                let value = match value {
                    FieldValue::Integer(value) => value,
                    _ => anyhow::bail!("选择值不是整数"),
                };
                self.set_copilot_number(field, value)
            }
            InputTarget::CopilotAdd { .. } | InputTarget::CopilotText(_) => {
                anyhow::bail!("该字段不支持选项选择")
            }
        }
    }

    fn open_field_editor(
        &mut self,
        title: String,
        current: FieldValue,
        editor: FieldEditor,
        target: InputTarget,
    ) {
        match editor {
            FieldEditor::Text => self.open_input(title, current.display(), target),
            FieldEditor::Select {
                options,
                allow_custom,
            } => self.open_select(title, current, options, allow_custom, target),
            FieldEditor::MultiSelect {
                options,
                allow_custom,
            } => self.open_multi_select(title, current, options, allow_custom, target),
            FieldEditor::Stage { allow_custom } => {
                self.open_stage_select(title, current, allow_custom, target)
            }
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(confirm) = self.confirm.take() {
                    match confirm.target {
                        ConfirmTarget::DeleteTask(index) => {
                            self.apply_config(|config| config.delete_task(index));
                            let len = self.config.as_ref().map_or(0, DailyConfig::len);
                            self.config_idx = self.config_idx.min(len.saturating_sub(1));
                        }
                        ConfirmTarget::DeleteVariant { task, variant } => {
                            self.apply_config(|config| config.delete_variant(task, variant));
                            let len = self
                                .config
                                .as_ref()
                                .map_or(0, |config| config.variant_count(task));
                            self.variant_idx = self.variant_idx.min(len.saturating_sub(1));
                        }
                        ConfirmTarget::DeleteCopilot(index) => {
                            self.apply_copilot_cache(|cache| cache.delete(index));
                            self.clamp_copilot_selection();
                        }
                        ConfirmTarget::ClearCopilotEntries => {
                            self.apply_copilot_cache(CopilotCache::clear_entries);
                            self.copilot_idx = 0;
                        }
                        ConfirmTarget::RunCommand(command) => {
                            self.start_command(command);
                        }
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.confirm = None,
            _ => {}
        }
    }

    fn create_stage_variant(&mut self) {
        let fields = self.current_task_fields();
        let Some(field) = fields.get(self.field_idx) else {
            return;
        };
        if field.key != "stage" {
            self.status_error("请先选中关卡字段，再按 v 创建活动变体".to_string());
            return;
        }
        let stage = self
            .task_field_value(field)
            .unwrap_or_else(|| field.default.clone());
        let FieldValue::String(stage) = stage else {
            self.status_error("当前关卡值不是文本".to_string());
            return;
        };
        let client = self
            .config
            .as_ref()
            .and_then(|config| config.param_value(self.config_idx, "client_type"))
            .and_then(|value| match value {
                FieldValue::String(value) if !value.trim().is_empty() => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| "Official".to_string());
        let task = self.config_idx;
        let mut new_index = None;
        if self.apply_config(|config| {
            new_index = Some(config.add_on_side_story_variant(task, &client, &stage)?);
            Ok(())
        }) {
            self.variant_idx = new_index.unwrap_or(0);
            self.field_idx = 0;
            self.screen = Screen::VariantEdit;
        }
    }

    fn edit_current_task_field(&mut self) {
        let fields = self.current_task_fields();
        let Some(field) = fields.get(self.field_idx).cloned() else {
            return;
        };
        let current = self
            .task_field_value(&field)
            .unwrap_or_else(|| field.default.clone());
        if matches!(current, FieldValue::Bool(_)) {
            let new_value = match current {
                FieldValue::Bool(value) => FieldValue::Bool(!value),
                _ => unreachable!(),
            };
            self.set_task_field(self.config_idx, &field, new_value);
        } else {
            let target = InputTarget::TaskField {
                task: self.config_idx,
                field: field.clone(),
            };
            self.open_field_editor(
                format!("编辑 {}", field.label),
                current,
                field.editor.clone(),
                target,
            );
        }
    }

    fn commit_input(&mut self) {
        let Some(dialog) = self.input.take() else {
            return;
        };
        let result = match dialog.target {
            InputTarget::TaskField { task, field } => {
                let current = self
                    .task_field_value(&field)
                    .unwrap_or_else(|| field.default.clone());
                FieldValue::parse_like(&dialog.value, Some(&current))
                    .and_then(|value| self.set_task_field_result(task, &field, value))
            }
            InputTarget::VariantField {
                task,
                variant,
                field,
            } => {
                let current = self
                    .variant_field_value(&field)
                    .unwrap_or_else(|| field.default.clone());
                FieldValue::parse_like(&dialog.value, Some(&current))
                    .and_then(|value| self.set_variant_field_result(task, variant, &field, value))
            }
            InputTarget::CopilotAdd { kind, destination } => {
                let input = dialog.value.trim().to_string();
                if input.is_empty() {
                    Err(anyhow::anyhow!("请输入作业代码、URI 或本地 JSON 路径"))
                } else {
                    self.start_copilot_import(kind, destination, input);
                    Ok(())
                }
            }
            InputTarget::CopilotNumber(field) => dialog
                .value
                .trim()
                .parse::<i64>()
                .context("请输入有效整数")
                .and_then(|value| self.set_copilot_number(field, value)),
            InputTarget::CopilotText(CopilotTextField::SupportName) => {
                self.copilot.support_unit_name = dialog.value.trim().to_string();
                self.save_copilot_settings();
                Ok(())
            }
        };
        if let Err(error) = result {
            self.status_error(error.to_string());
        }
    }

    fn set_copilot_number(&mut self, field: CopilotNumberField, value: i64) -> anyhow::Result<()> {
        match field {
            CopilotNumberField::FormationIndex if !(0..=4).contains(&value) => {
                anyhow::bail!("编队编号必须为 0-4")
            }
            CopilotNumberField::SupportUsage if !(0..=3).contains(&value) => {
                anyhow::bail!("助战模式必须为 0-3")
            }
            CopilotNumberField::LoopTimes if value <= 0 => anyhow::bail!("循环次数必须大于 0"),
            CopilotNumberField::FormationIndex => self.copilot.formation_index = value,
            CopilotNumberField::SupportUsage => self.copilot.support_unit_usage = value,
            CopilotNumberField::LoopTimes => self.copilot.loop_times = value,
        }
        self.save_copilot_settings();
        Ok(())
    }

    fn open_input(&mut self, title: String, value: String, target: InputTarget) {
        self.input = Some(InputDialog {
            title,
            value,
            target,
        });
    }

    fn dynamic_stage_options(&self) -> Vec<SelectOption> {
        let client = self
            .config
            .as_ref()
            .and_then(|config| config.param_value(self.config_idx, "client_type"))
            .and_then(|value| match value {
                FieldValue::String(value) if !value.trim().is_empty() => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| "Official".to_string());
        let mut options = stage_options();
        let mut seen: HashSet<String> = options
            .iter()
            .filter_map(|option| match &option.value {
                FieldValue::String(value) => Some(value.clone()),
                _ => None,
            })
            .collect();
        for entry in self.stage_catalog.entries_for(&client) {
            if seen.insert(entry.value.clone()) {
                options.push(SelectOption {
                    value: FieldValue::String(entry.value),
                    label: entry.label,
                    description: entry.description,
                });
            }
        }
        options
    }

    fn open_stage_select(
        &mut self,
        title: String,
        current: FieldValue,
        allow_custom: bool,
        target: InputTarget,
    ) {
        self.open_select_with(
            title,
            current,
            self.dynamic_stage_options(),
            SelectBehavior {
                allow_custom,
                refreshable: true,
                multi: false,
            },
            Vec::new(),
            target,
        );
    }

    fn open_multi_select(
        &mut self,
        title: String,
        current: FieldValue,
        options: Vec<SelectOption>,
        allow_custom: bool,
        target: InputTarget,
    ) {
        let selected_values = match &current {
            FieldValue::IntegerArray(values) => {
                values.iter().copied().map(FieldValue::Integer).collect()
            }
            FieldValue::StringArray(values) => {
                values.iter().cloned().map(FieldValue::String).collect()
            }
            _ => Vec::new(),
        };
        self.open_select_with(
            title,
            current,
            options,
            SelectBehavior {
                allow_custom,
                refreshable: false,
                multi: true,
            },
            selected_values,
            target,
        );
    }

    fn open_select(
        &mut self,
        title: String,
        current: FieldValue,
        options: Vec<SelectOption>,
        allow_custom: bool,
        target: InputTarget,
    ) {
        self.open_select_with(
            title,
            current,
            options,
            SelectBehavior {
                allow_custom,
                refreshable: false,
                multi: false,
            },
            Vec::new(),
            target,
        );
    }

    fn open_select_with(
        &mut self,
        title: String,
        current: FieldValue,
        options: Vec<SelectOption>,
        behavior: SelectBehavior,
        selected_values: Vec<FieldValue>,
        target: InputTarget,
    ) {
        let selected = options
            .iter()
            .position(|option| option.value == current)
            .unwrap_or(0);
        self.select = Some(SelectDialog {
            title,
            options,
            selected,
            allow_custom: behavior.allow_custom,
            refreshable: behavior.refreshable,
            multi: behavior.multi,
            selected_values,
            current,
            target,
        });
    }

    fn refresh_stage_catalog(&mut self) {
        if self.stage_refreshing {
            if self.can_report_stage_refresh_status() {
                self.status_text = "活动关卡目录正在刷新…".to_string();
            }
            return;
        }
        match StageCatalog::spawn_refresh(self.stage_refresh_tx.clone()) {
            Ok(()) => {
                self.stage_refreshing = true;
                if self.can_report_stage_refresh_status() {
                    self.status_text = "正在刷新活动关卡目录…".to_string();
                }
            }
            Err(error) => self.status_error(format!("启动活动关卡刷新线程失败: {error}")),
        }
    }

    fn can_report_stage_refresh_status(&self) -> bool {
        self.phase == TaskPhase::Idle && !self.last_failed
    }

    pub fn current_task_fields(&self) -> Vec<FieldSpec> {
        if self.editor_section() == EditorSection::Variants {
            return Vec::new();
        }
        let task_type = self
            .config
            .as_ref()
            .and_then(|config| config.task_type(self.config_idx))
            .unwrap_or_default();
        task_fields(&task_type, self.editor_section() == EditorSection::Advanced)
    }

    pub fn field_display(&self, field: &FieldSpec, value: &FieldValue) -> String {
        if matches!(field.editor, FieldEditor::Stage { .. }) {
            option_label(&self.dynamic_stage_options(), value).unwrap_or_else(|| value.display())
        } else {
            field.display_value(value)
        }
    }

    pub fn task_field_value(&self, field: &FieldSpec) -> Option<FieldValue> {
        let config = self.config.as_ref()?;
        match field.scope {
            FieldScope::Task => config.task_value(self.config_idx, field.key),
            FieldScope::Param => config.param_value(self.config_idx, field.key),
            _ => None,
        }
    }

    fn set_task_field(&mut self, task: usize, field: &FieldSpec, value: FieldValue) {
        if let Err(error) = self.set_task_field_result(task, field, value) {
            self.status_error(error.to_string());
        }
    }

    fn set_task_field_result(
        &mut self,
        task: usize,
        field: &FieldSpec,
        value: FieldValue,
    ) -> anyhow::Result<()> {
        validate_field_value(field.key, &value)?;
        let config = self.config.as_mut().context("daily 配置未加载")?;
        match field.scope {
            FieldScope::Task => config.set_task_value(task, field.key, value),
            FieldScope::Param => config.set_param_value(task, field.key, value),
            _ => anyhow::bail!("字段范围无效"),
        }?;
        self.last_failed = false;
        Ok(())
    }

    pub fn variant_field_value(&self, field: &FieldSpec) -> Option<FieldValue> {
        let config = self.config.as_ref()?;
        match field.scope {
            FieldScope::VariantParam => {
                config.variant_param_value(self.config_idx, self.variant_idx, field.key)
            }
            FieldScope::VariantCondition => {
                config.variant_condition_value(self.config_idx, self.variant_idx, field.key)
            }
            _ => None,
        }
    }

    fn set_variant_field_result(
        &mut self,
        task: usize,
        variant: usize,
        field: &FieldSpec,
        value: FieldValue,
    ) -> anyhow::Result<()> {
        validate_field_value(field.key, &value)?;
        let config = self.config.as_mut().context("daily 配置未加载")?;
        match field.scope {
            FieldScope::VariantParam => {
                config.set_variant_param_value(task, variant, field.key, value)
            }
            FieldScope::VariantCondition => {
                config.set_variant_condition_value(task, variant, field.key, value)
            }
            _ => anyhow::bail!("字段范围无效"),
        }?;
        self.last_failed = false;
        Ok(())
    }

    fn apply_config<F>(&mut self, operation: F) -> bool
    where
        F: FnOnce(&mut DailyConfig) -> anyhow::Result<()>,
    {
        let result = self
            .config
            .as_mut()
            .context("daily 配置未加载")
            .and_then(operation);
        match result {
            Ok(()) => {
                self.last_failed = false;
                true
            }
            Err(error) => {
                self.status_error(error.to_string());
                false
            }
        }
    }

    fn apply_copilot_cache<F>(&mut self, operation: F) -> bool
    where
        F: FnOnce(&mut CopilotCache) -> anyhow::Result<()>,
    {
        let result = self
            .copilot_cache
            .as_mut()
            .context("作业列表未加载")
            .and_then(operation);
        match result {
            Ok(()) => {
                self.last_failed = false;
                true
            }
            Err(error) => {
                self.status_error(error.to_string());
                false
            }
        }
    }

    fn start_copilot_import(
        &mut self,
        kind: ImportKind,
        destination: ImportDestination,
        input: String,
    ) {
        if self.copilot_importing {
            self.status_error("已有作业正在导入".to_string());
            return;
        }
        let Some(cache) = self.copilot_cache.as_ref() else {
            self.status_error(
                self.copilot_error
                    .clone()
                    .unwrap_or_else(|| "作业列表未加载".to_string()),
            );
            return;
        };
        let files_dir = cache.files_dir().to_path_buf();
        let tx = self.copilot_import_tx.clone();
        self.copilot_importing = true;
        self.copilot_import_progress = None;
        self.last_failed = false;
        self.status_text = match kind {
            ImportKind::Single => "正在导入单作业…".to_string(),
            ImportKind::Set => "正在导入作业集…".to_string(),
        };
        if let Err(error) = thread::Builder::new()
            .name("maatui-copilot-import".to_string())
            .spawn(move || {
                let result = import_source(&input, kind, &files_dir, |progress| {
                    let _ = tx.send(CopilotImportEvent::Progress(progress));
                })
                .map_err(|error| format!("{error:#}"));
                let _ = tx.send(CopilotImportEvent::Finished {
                    destination,
                    result,
                });
            })
        {
            self.copilot_importing = false;
            self.status_error(format!("启动作业导入线程失败: {error}"));
        }
    }

    fn poll_copilot_import(&mut self) {
        while let Ok(event) = self.copilot_import_rx.try_recv() {
            match event {
                CopilotImportEvent::Progress(progress) => {
                    if self.can_report_scope_status(LogScope::Copilot) {
                        self.status_text = if progress.total > 0 {
                            format!(
                                "导入作业 {}/{} · {}",
                                progress.completed, progress.total, progress.label
                            )
                        } else {
                            progress.label.clone()
                        };
                    }
                    self.copilot_import_progress = Some(progress);
                }
                CopilotImportEvent::Finished {
                    destination,
                    result,
                } => {
                    self.copilot_importing = false;
                    self.copilot_import_progress = None;
                    let result = result
                        .map_err(anyhow::Error::msg)
                        .and_then(|report| self.finish_copilot_import(destination, report));
                    if let Err(error) = result {
                        self.background_status_error_to(LogScope::Copilot, error.to_string());
                    }
                }
            }
        }
    }

    fn finish_copilot_import(
        &mut self,
        destination: ImportDestination,
        mut report: ImportReport,
    ) -> anyhow::Result<()> {
        if destination == ImportDestination::CurrentSingle {
            let added = report.entries.len();
            let mut entry = report
                .entries
                .drain(..)
                .next()
                .context("作业未生成可运行模式")?;
            entry.origin = crate::copilot::CopilotOrigin::Single;
            self.copilot_cache
                .as_mut()
                .context("作业列表未加载")?
                .replace_current_single(Some(entry))?;
            self.current_single_supported_modes = self
                .copilot_cache
                .as_ref()
                .and_then(|cache| cache.current_single_supported_modes().ok());
            self.copilot_section_idx = 0;
            if self.can_report_scope_status(LogScope::Copilot) {
                self.status_text = if added > 1 {
                    "已替换当前单作业；支持普通与突袭，按 Space 切换本次运行模式".to_string()
                } else {
                    "已替换当前单作业".to_string()
                };
                self.last_failed = false;
            }
            return Ok(());
        }

        for entry in &mut report.entries {
            if report.set_id.is_none() {
                entry.origin = crate::copilot::CopilotOrigin::Single;
            }
        }
        let added = report.entries.len();
        let first = if added > 0 {
            Some(
                self.copilot_cache
                    .as_mut()
                    .context("作业列表未加载")?
                    .append(report.entries)?,
            )
        } else {
            None
        };
        if let Some(index) = first {
            self.copilot_section_idx = 1;
            self.copilot_idx = index;
        }
        if let Some(id) = report.set_id {
            let label = report
                .set_name
                .as_deref()
                .map_or_else(|| format!("#{id}"), |name| format!("{name} (#{id})"));
            self.push_log_to(
                LogScope::Copilot,
                LogLevel::System,
                format!("作业集：{label}"),
            );
        }
        if let Some(description) = report.set_description {
            self.push_log_to(LogScope::Copilot, LogLevel::Plain, description);
        }
        for error in &report.errors {
            self.push_log_to(
                LogScope::Copilot,
                LogLevel::Warn,
                format!("作业导入失败：{error}"),
            );
        }
        if self.can_report_scope_status(LogScope::Copilot) {
            self.status_text = if report.errors.is_empty() {
                format!("已添加 {added} 个作业")
            } else {
                format!("已添加 {added} 个作业，{} 个失败", report.errors.len())
            };
            self.last_failed = added == 0 && !report.errors.is_empty();
        }
        Ok(())
    }

    fn reload_config(&mut self) {
        match DailyConfig::load_default() {
            Ok(config) => {
                self.config = Some(config);
                self.config_error = None;
                self.last_failed = false;
                let len = self.config.as_ref().map_or(0, DailyConfig::len);
                self.config_idx = self.config_idx.min(len.saturating_sub(1));
            }
            Err(error) => {
                self.config = None;
                self.config_error = Some(error.to_string());
                self.status_error(error.to_string());
            }
        }
    }

    fn start_current_single_copilot(&mut self) {
        let progress = self.copilot_cache.as_ref().and_then(|cache| {
            let entry = cache.current_single()?;
            Some(RunProgress {
                current: 1,
                total: self.copilot.loop_times.max(1) as usize,
                label: format!("{} · {}", entry.stage_name, entry.display_name()),
            })
        });
        let command = match self.current_single_copilot_command() {
            Ok(command) => command,
            Err(error) => {
                self.status_error(error.to_string());
                return;
            }
        };
        self.ensure_tile_pos_aliases();
        self.run_progress = progress;
        self.start_command(command);
    }

    fn current_single_copilot_command(&self) -> anyhow::Result<TaskCommand> {
        let cache = self.copilot_cache.as_ref().context("作业缓存未加载")?;
        let entry = cache.current_single().context("请先搜索当前单作业")?;
        let path = cache
            .current_single_path()
            .context("无法解析作业文件路径")?;
        self.single_copilot_command(entry, path)
    }

    fn start_selected_copilot(&mut self) {
        let progress = self.selected_copilot_index().and_then(|index| {
            let entry = self.copilot_cache.as_ref()?.entry(index)?;
            Some(RunProgress {
                current: 1,
                total: self.copilot.loop_times.max(1) as usize,
                label: format!("{} · {}", entry.stage_name, entry.display_name()),
            })
        });
        let command = match self.selected_copilot_command() {
            Ok(command) => command,
            Err(error) => {
                self.status_error(error.to_string());
                return;
            }
        };
        self.ensure_tile_pos_aliases();
        self.run_progress = progress;
        self.start_command(command);
    }

    fn selected_copilot_command(&self) -> anyhow::Result<TaskCommand> {
        let cache = self.copilot_cache.as_ref().context("作业列表未加载")?;
        let index = self
            .selected_copilot_index()
            .context("请选择要运行的作业")?;
        let entry = cache.entry(index).context("请选择要运行的作业")?;
        let path = cache
            .resolved_file_path(index)
            .context("无法解析作业文件路径")?;
        self.single_copilot_command(entry, path)
    }

    fn single_copilot_command(
        &self,
        entry: &crate::copilot::CopilotEntry,
        path: PathBuf,
    ) -> anyhow::Result<TaskCommand> {
        if !path.is_file() {
            anyhow::bail!("作业文件不存在: {}", path.display());
        }
        let mut args = vec![
            "copilot".to_string(),
            path.display().to_string(),
            "--raid".to_string(),
            if entry.is_raid { "raid" } else { "normal" }.to_string(),
        ];
        self.append_single_copilot_options(&mut args);
        Ok(TaskCommand::copilot(args))
    }

    fn start_copilot_batch(&mut self) {
        let Some(cache) = self.copilot_cache.as_ref() else {
            self.status_error("作业列表未加载".to_string());
            return;
        };
        let indices = cache.enabled_indices();
        let options = CopilotRunOptions {
            formation_index: self.copilot.formation_index,
            use_sanity_potion: self.copilot.use_sanity_potion,
            add_trust: self.copilot.add_trust,
            ignore_requirements: self.copilot.ignore_requirements,
            support_unit_usage: self.copilot.support_unit_usage,
            support_unit_name: self.copilot.support_unit_name.clone(),
        };
        let BatchTask { path, indices } = match write_batch_task(cache, indices, &options) {
            Ok(task) => task,
            Err(error) => {
                self.status_error(error.to_string());
                return;
            }
        };

        self.ensure_tile_pos_aliases();
        let command = TaskCommand::copilot_batch(&path.display().to_string());
        if self.start_command(command) {
            let total = indices.len();
            let current_name = indices
                .first()
                .and_then(|&index| self.copilot_cache.as_ref()?.entry(index))
                .map(|entry| format!("{} · {}", entry.stage_name, entry.display_name()))
                .unwrap_or_else(|| "未知作业".to_string());
            self.status_text = format!("作业集运行中 · 1/{total} · {current_name}");
            self.run_progress = Some(RunProgress {
                current: 1,
                total,
                label: current_name,
            });
            self.copilot_batch = Some(CopilotBatchState::new(path, indices));
        } else {
            remove_batch_task(&path);
        }
    }

    fn append_single_copilot_options(&self, args: &mut Vec<String>) {
        if self.copilot.formation {
            args.push("--formation".to_string());
        }
        if self.copilot.formation_index > 0 {
            args.extend([
                "--formation-index".to_string(),
                self.copilot.formation_index.to_string(),
            ]);
        }
        if self.copilot.use_sanity_potion {
            args.push("--use-sanity-potion".to_string());
        }
        if self.copilot.add_trust {
            args.push("--add-trust".to_string());
        }
        if self.copilot.ignore_requirements {
            args.push("--ignore-requirements".to_string());
        }
        if self.copilot.support_unit_usage > 0 {
            args.extend([
                "--support-unit-usage".to_string(),
                self.copilot.support_unit_usage.to_string(),
            ]);
        }
        if !self.copilot.support_unit_name.is_empty() {
            args.extend([
                "--support-unit-name".to_string(),
                self.copilot.support_unit_name.clone(),
            ]);
        }
        args.extend([
            "--loop-times".to_string(),
            self.copilot.loop_times.to_string(),
            "--batch".to_string(),
            "-v".to_string(),
        ]);
    }

    fn start_command(&mut self, command: TaskCommand) -> bool {
        self.active_log_scope = log_scope_for_command(&command);
        let buffer = self.log_buffer_mut(self.active_log_scope);
        buffer.scroll = u16::MAX;
        buffer.auto_scroll = true;
        self.last_failed = false;
        self.saw_outdated_resource_error = false;
        self.saw_ocr_to_t0_error = false;
        if command.tracks_daily_progress() {
            let (tasks, total) = match command.daily_single_index {
                Some(index) => {
                    let tasks = self
                        .config
                        .as_ref()
                        .and_then(|config| config.task_summary(index))
                        .into_iter()
                        .collect::<Vec<_>>();
                    // 用户主动单独执行，即使原配置关闭也按单个任务计。
                    (tasks, 1)
                }
                None => {
                    let tasks = self
                        .config
                        .as_ref()
                        .map(DailyConfig::task_summaries)
                        .unwrap_or_default();
                    let total = tasks.iter().filter(|task| task.enabled).count();
                    (tasks, total)
                }
            };
            self.run_progress = Some(RunProgress {
                current: 0,
                total,
                label: "等待任务开始".to_string(),
            });
            self.daily_run = Some(DailyRunState { tasks, total });
        }
        self.temp_task_file = command.cleanup.clone();
        self.pending_resource_check = is_resource_update_command(&command);
        self.resource_version_before = if self.pending_resource_check {
            read_hot_update_version()
        } else {
            None
        };
        self.push_log(LogLevel::System, format!("启动: {}", command.display()));
        self.status_text = format!("正在启动{}…", command.label);

        match RunningTask::spawn(&command) {
            Ok(task) => {
                self.task = Some(task);
                self.phase = TaskPhase::Running;
                self.started_at = Some(Instant::now());
                self.animation_started = Instant::now();
                self.active_label = command.label;
                self.status_text = format!("{}运行中", self.active_label);
                true
            }
            Err(error) => {
                self.pending_resource_check = false;
                self.resource_version_before = None;
                self.remove_temp_task_file();
                self.push_log(LogLevel::Error, error);
                self.status_text = "启动失败".to_string();
                self.last_failed = true;
                self.run_progress = None;
                self.daily_run = None;
                false
            }
        }
    }

    fn request_stop(&mut self) {
        if let Some(task) = self.task.as_mut()
            && !task.is_finished()
            && !task.stop_requested()
        {
            task.request_stop();
            self.phase = TaskPhase::Stopping;
            self.status_text = format!("正在停止{}…", self.active_label);
            self.push_log(LogLevel::Warn, "已发送停止信号 (SIGTERM)");
        }
    }

    fn request_quit(&mut self) {
        match self.phase {
            TaskPhase::Idle => self.should_quit = true,
            TaskPhase::Running | TaskPhase::Stopping => {
                self.request_stop();
                self.should_quit = true;
                self.push_log(LogLevel::System, "退出前停止任务…");
            }
        }
    }

    pub fn tick(&mut self) {
        self.poll_stage_refresh();
        self.poll_copilot_import();
        let events = self
            .task
            .as_mut()
            .map_or_else(Vec::new, RunningTask::poll_events);
        for event in events {
            match event {
                RunnerEvent::Line { level, text } => self.push_log(level, text),
                RunnerEvent::DailyTaskStarted { task_id, taskchain } => {
                    self.on_daily_task_started(task_id, &taskchain)
                }
                RunnerEvent::CopilotStageSucceeded => self.on_copilot_stage_succeeded(),
                RunnerEvent::ProgressFailed(error) => {
                    let message = format!("读取 MaaCore 运行进度失败，已停止任务: {error}");
                    if let Some(batch) = self.copilot_batch.as_mut() {
                        batch.set_abort_error(message.clone());
                    }
                    self.request_stop();
                    self.status_error(message);
                }
                RunnerEvent::Exited { code, stopped } => self.on_exited(code, stopped),
            }
        }
    }

    fn poll_stage_refresh(&mut self) {
        while let Ok(event) = self.stage_refresh_rx.try_recv() {
            self.stage_refreshing = false;
            match event {
                StageRefreshEvent::Updated {
                    catalog,
                    tasks_updated,
                } => {
                    self.stage_catalog = catalog;
                    if self
                        .select
                        .as_ref()
                        .is_some_and(|select| select.refreshable)
                    {
                        let current = self.select.as_ref().map(|select| select.current.clone());
                        let options = self.dynamic_stage_options();
                        if let Some(select) = self.select.as_mut() {
                            select.selected = current
                                .as_ref()
                                .and_then(|current| {
                                    options.iter().position(|option| option.value == *current)
                                })
                                .unwrap_or(0);
                            select.options = options;
                        }
                    }
                    if self.can_report_stage_refresh_status() {
                        self.status_text = if tasks_updated {
                            "活动关卡与导航资源已热更新".to_string()
                        } else {
                            "活动关卡已热更新（导航资源更新失败）".to_string()
                        };
                    }
                }
                StageRefreshEvent::Failed(error) => {
                    if self.can_report_stage_refresh_status() {
                        self.status_text = format!("活动关卡热更新失败，使用本地缓存：{error}");
                    }
                }
            }
        }
    }

    fn on_daily_task_started(&mut self, task_id: usize, taskchain: &str) {
        let Some(state) = self.daily_run.as_ref() else {
            return;
        };
        let total = state.total;
        // 防御：当前进度不超过分母（taskid 语义在不同 maa 版本下可能保留关闭任务的序号）。
        let current = task_id.min(total);
        let index = task_id.saturating_sub(1);
        let enabled_index = current.saturating_sub(1);
        // 优先按实际启用任务顺序取名称，避免关闭任务穿插及同类型任务重复时错位；
        // 若回调使用原配置索引，则退回原始索引匹配，最后使用任务类型标签。
        let label = state
            .tasks
            .iter()
            .filter(|task| task.enabled)
            .nth(enabled_index)
            .filter(|task| task.task_type == taskchain)
            .or_else(|| {
                state
                    .tasks
                    .get(index)
                    .filter(|task| task.task_type == taskchain)
            })
            .map(|task| task.name.clone())
            .unwrap_or_else(|| task_type_label(taskchain).to_string());
        self.run_progress = Some(RunProgress {
            current,
            total,
            label,
        });
    }

    fn on_copilot_stage_succeeded(&mut self) {
        if self.copilot_batch.is_none() {
            if let Some(progress) = self.run_progress.as_mut()
                && progress.current < progress.total
            {
                progress.current += 1;
            }
            return;
        }
        let Some(completion) = self
            .copilot_batch
            .as_ref()
            .and_then(CopilotBatchState::current_completion)
        else {
            return;
        };
        let index = completion.index;
        let total = completion.total;
        let completed = completion.completed;
        let name = self
            .copilot_cache
            .as_ref()
            .and_then(|cache| cache.entry(index))
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| format!("作业 {}", index + 1));
        let result = self
            .copilot_cache
            .as_mut()
            .context("作业列表未加载")
            .and_then(|cache| cache.set_enabled(index, false));
        match result {
            Ok(()) => {
                if let Some(batch) = self.copilot_batch.as_mut() {
                    batch.mark_success();
                }
                self.push_log(
                    LogLevel::Success,
                    format!("已完成并关闭 [{completed}/{total}] {name}"),
                );
                self.status_text = if completed < total {
                    let next = self
                        .copilot_batch
                        .as_ref()
                        .and_then(|batch| match batch.position()? {
                            BatchPosition::Running { index, .. } => Some(index),
                            BatchPosition::Finishing { .. } => None,
                        })
                        .and_then(|index| self.copilot_cache.as_ref()?.entry(index))
                        .map(|entry| format!("{} · {}", entry.stage_name, entry.display_name()))
                        .unwrap_or_else(|| "未知作业".to_string());
                    self.run_progress = Some(RunProgress {
                        current: completed + 1,
                        total,
                        label: next.clone(),
                    });
                    format!("作业集运行中 · {}/{total} · {next}", completed + 1)
                } else {
                    if let Some(progress) = self.run_progress.as_mut() {
                        progress.current = total;
                        progress.label = "全部作业已完成，正在收尾".to_string();
                    }
                    format!("作业集运行中 · {total}/{total} · 正在收尾")
                };
            }
            Err(error) => {
                let message = format!("保存成功作业状态失败，已停止批次: {error}");
                if let Some(batch) = self.copilot_batch.as_mut() {
                    batch.set_abort_error(message.clone());
                }
                self.request_stop();
                self.status_error(message);
            }
        }
    }

    fn on_exited(&mut self, code: Option<i32>, stopped: bool) {
        self.remove_temp_task_file();
        let code_text = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        let check_resource = self.pending_resource_check;
        let version_before = self.resource_version_before.take();
        self.pending_resource_check = false;
        let mut batch_state_error = None;
        let mut batch_abort_error = None;
        if let Some(batch) = self.copilot_batch.take() {
            let exit = batch.finish(code, stopped);
            batch_abort_error = exit.abort_error;
            if !exit.reconcile_indices.is_empty() {
                let result = self
                    .copilot_cache
                    .as_mut()
                    .context("作业列表未加载")
                    .and_then(|cache| cache.set_enabled_many(&exit.reconcile_indices, false));
                if let Err(error) = result {
                    batch_state_error = Some(error.to_string());
                } else {
                    self.push_log(
                        LogLevel::Warn,
                        format!(
                            "批次整体成功；已补记 {} 个未收到逐关日志的成功项",
                            exit.reconcile_indices.len()
                        ),
                    );
                }
            }
            remove_batch_task(&exit.task_path);
        }

        if stopped && batch_abort_error.is_none() {
            self.push_log(
                LogLevel::Warn,
                format!("{}已停止 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已停止 · exit {code_text}");
            self.last_failed = false;
        } else if code == Some(0) && batch_state_error.is_none() {
            self.push_log(
                LogLevel::Success,
                format!("{}完成 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已完成 · exit {code_text}");
            self.last_failed = false;
            if check_resource {
                self.report_resource_update_result(version_before.as_ref());
            }
        } else {
            self.push_log(
                LogLevel::Error,
                format!("{}异常结束 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("失败 · exit {code_text}");
            self.last_failed = true;
            if let Some(error) = batch_abort_error {
                self.push_log(LogLevel::Error, error);
            }
            if let Some(error) = batch_state_error {
                self.push_log(LogLevel::Error, format!("保存作业列表失败: {error}"));
            }
            if self.saw_ocr_to_t0_error {
                self.push_log(LogLevel::System, OCR_TO_T0_HINT);
            } else if self.saw_outdated_resource_error {
                self.push_log(LogLevel::System, OUTDATED_RESOURCE_HINT);
            }
        }
        self.task = None;
        self.phase = TaskPhase::Idle;
        self.started_at = None;
        self.active_label.clear();
        self.run_progress = None;
        self.daily_run = None;
        self.saw_outdated_resource_error = false;
        self.saw_ocr_to_t0_error = false;
        let buffer = self.log_buffer_mut(self.active_log_scope);
        buffer.auto_scroll = true;
        buffer.scroll = u16::MAX;
    }

    fn report_resource_update_result(&mut self, before: Option<&HotUpdateVersion>) {
        match read_hot_update_version() {
            Some(after) => {
                let summary = after.summary();
                if before.is_some_and(|before| before == &after) {
                    self.push_log(
                        LogLevel::Warn,
                        "热更新报告成功，但本地 version.json 未变化，可能未拉到新提交",
                    );
                    self.status_text = format!("{summary} · 未变化");
                } else {
                    self.push_log(LogLevel::System, &summary);
                    self.status_text = summary;
                }
            }
            None => {
                self.push_log(
                    LogLevel::Warn,
                    "热更新完成，但无法读取本地 version.json；请检查 `maa dir hot-update`",
                );
            }
        }
        self.ensure_tile_pos_aliases();
    }

    fn ensure_tile_pos_aliases(&mut self) {
        let report = tile_alias::ensure_stage_code_aliases();
        if let Some(summary) = report.summary() {
            let level = if report.errors.is_empty() {
                LogLevel::System
            } else {
                LogLevel::Warn
            };
            self.push_log(level, summary);
        }
    }

    /// 删除单任务执行生成的临时任务文件；删除失败仅记录日志，不阻断。
    fn remove_temp_task_file(&mut self) {
        if let Some(path) = self.temp_task_file.take()
            && let Err(error) = fs::remove_file(&path)
        {
            self.push_log(
                LogLevel::Warn,
                format!("清理临时任务文件失败 {}: {error}", path.display()),
            );
        }
    }

    pub fn cleanup(&mut self) {
        if let Some(task) = self.task.take() {
            task.force_cleanup();
        }
        if let Some(batch) = self.copilot_batch.take() {
            remove_batch_task(batch.task_path());
        }
        self.remove_temp_task_file();
        self.phase = TaskPhase::Idle;
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = (self.animation_started.elapsed().as_millis() / 80) as usize;
        FRAMES[frame % FRAMES.len()]
    }

    fn scroll_logs_up(&mut self, amount: u16) {
        let scope = self.visible_log_scope();
        let buffer = self.log_buffer_mut(scope);
        buffer.auto_scroll = false;
        buffer.scroll = buffer.scroll.saturating_sub(amount);
    }

    fn scroll_logs_down(&mut self, amount: u16) {
        let scope = self.visible_log_scope();
        let buffer = self.log_buffer_mut(scope);
        buffer.scroll = buffer.scroll.saturating_add(amount);
    }

    fn can_report_scope_status(&self, scope: LogScope) -> bool {
        if self.phase != TaskPhase::Idle {
            return false;
        }
        matches!(
            (scope, self.screen),
            (
                LogScope::Daily,
                Screen::Daily
                    | Screen::Config
                    | Screen::AddTask
                    | Screen::TaskEdit
                    | Screen::VariantList
                    | Screen::VariantEdit
            ) | (LogScope::Copilot, Screen::Copilot)
                | (LogScope::Update, Screen::Update)
        )
    }

    fn background_status_error_to(&mut self, scope: LogScope, message: String) {
        if self.can_report_scope_status(scope) {
            self.status_text = format!("错误: {message}");
            self.last_failed = true;
        }
        self.push_log_to(scope, LogLevel::Error, message);
    }

    fn status_error_to(&mut self, scope: LogScope, message: String) {
        self.status_text = format!("错误: {message}");
        self.last_failed = true;
        self.push_log_to(scope, LogLevel::Error, message);
    }

    fn status_error(&mut self, message: String) {
        let scope = self.visible_log_scope();
        self.status_error_to(scope, message);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn task_fields(task_type: &str, advanced: bool) -> Vec<FieldSpec> {
    let mut fields = if advanced {
        Vec::new()
    } else {
        vec![
            field(
                "name",
                "名称",
                FieldScope::Task,
                FieldValue::String(String::new()),
            ),
            field("enable", "启用", FieldScope::Param, FieldValue::Bool(true)),
        ]
    };
    let definitions: &[(&str, &str, FieldValue, bool)] = match task_type {
        "StartUp" => &[
            ("client_type", "客户端", string("Official"), false),
            (
                "start_game_enabled",
                "自动启动游戏",
                FieldValue::Bool(true),
                false,
            ),
            ("account_name", "账号名称", string(""), true),
        ],
        "Recruit" => &[
            ("times", "招募次数", FieldValue::Integer(4), false),
            ("select", "选择星级", integers(&[4]), false),
            ("confirm", "确认星级", integers(&[3, 4]), false),
            ("refresh", "刷新三星标签", FieldValue::Bool(false), true),
            ("expedite", "使用加急许可", FieldValue::Bool(false), true),
            ("skip_robot", "跳过支援机械", FieldValue::Bool(true), true),
            ("first_tags", "首选标签", strings(&[]), true),
            (
                "extra_tags_mode",
                "额外标签模式",
                FieldValue::Integer(0),
                true,
            ),
            ("server", "统计服务器", string("CN"), true),
        ],
        "Fight" => &[
            ("stage", "关卡", string(""), false),
            ("times", "战斗次数", FieldValue::Integer(0), false),
            ("medicine", "理智药", FieldValue::Integer(0), false),
            (
                "medicine_expire_days",
                "临期理智药",
                FieldValue::Integer(0),
                false,
            ),
            ("series", "连战次数", FieldValue::Integer(1), false),
            ("stone", "碎石数量", FieldValue::Integer(0), true),
            (
                "report_to_penguin",
                "汇报企鹅物流",
                FieldValue::Bool(false),
                true,
            ),
            ("penguin_id", "企鹅物流 ID", string(""), true),
            (
                "report_to_yituliu",
                "汇报一图流",
                FieldValue::Bool(false),
                true,
            ),
            ("yituliu_id", "一图流 ID", string(""), true),
            ("client_type", "崩溃重连客户端", string(""), true),
            ("DrGrandet", "节省理智碎石", FieldValue::Bool(false), true),
        ],
        "Infrast" => &[
            (
                "facility",
                "设施顺序",
                strings(&[
                    "Mfg",
                    "Trade",
                    "Control",
                    "Power",
                    "Reception",
                    "Office",
                    "Dorm",
                ]),
                false,
            ),
            ("drones", "无人机用途", string("Money"), false),
            ("mode", "换班模式", FieldValue::Integer(0), false),
            ("threshold", "心情阈值", FieldValue::Float(0.3), true),
            ("replenish", "源石碎片补货", FieldValue::Bool(false), true),
            (
                "dorm_trust_enabled",
                "宿舍补信赖",
                FieldValue::Bool(false),
                true,
            ),
            ("filename", "自定义排班文件", string(""), true),
            ("plan_index", "排班方案序号", FieldValue::Integer(0), true),
        ],
        "Mall" => &[
            ("visit_friends", "访问好友", FieldValue::Bool(true), false),
            ("shopping", "信用购物", FieldValue::Bool(true), false),
            ("buy_first", "优先购买", strings(&[]), false),
            ("blacklist", "购物黑名单", strings(&[]), false),
            ("credit_fight", "信用助战", FieldValue::Bool(false), true),
            (
                "only_buy_discount",
                "仅买折扣",
                FieldValue::Bool(false),
                true,
            ),
            (
                "reserve_max_credit",
                "保留信用",
                FieldValue::Bool(false),
                true,
            ),
            ("formation_index", "助战编队", FieldValue::Integer(0), true),
        ],
        "Award" => &[
            ("award", "任务奖励", FieldValue::Bool(true), false),
            ("mail", "邮件奖励", FieldValue::Bool(false), false),
            ("recruit", "限定池单抽", FieldValue::Bool(false), true),
            ("orundum", "幸运墙", FieldValue::Bool(false), true),
            ("mining", "限时开采", FieldValue::Bool(false), true),
            ("specialaccess", "特殊月卡", FieldValue::Bool(false), true),
        ],
        "CloseDown" => &[("client_type", "客户端", string("Official"), false)],
        "Copilot" => &[
            ("filename", "作业文件", string(""), false),
            ("loop_times", "循环次数", FieldValue::Integer(1), false),
            ("formation", "自动编队", FieldValue::Bool(false), false),
            ("formation_index", "编队编号", FieldValue::Integer(0), true),
            (
                "use_sanity_potion",
                "使用理智药",
                FieldValue::Bool(false),
                true,
            ),
            ("add_trust", "补低信赖干员", FieldValue::Bool(false), true),
            (
                "ignore_requirements",
                "忽略练度要求",
                FieldValue::Bool(false),
                true,
            ),
            (
                "support_unit_usage",
                "助战模式",
                FieldValue::Integer(0),
                true,
            ),
            ("support_unit_name", "指定助战干员", string(""), true),
        ],
        _ => &[],
    };
    fields.extend(
        definitions
            .iter()
            .filter(|(_, _, _, is_advanced)| *is_advanced == advanced)
            .map(|(key, label, default, _)| field(key, label, FieldScope::Param, default.clone())),
    );
    fields
}

pub fn variant_fields() -> Vec<FieldSpec> {
    vec![
        field(
            "type",
            "条件类型",
            FieldScope::VariantCondition,
            string("Always"),
        ),
        field(
            "client",
            "活动客户端",
            FieldScope::VariantCondition,
            string("Official"),
        ),
        field(
            "start",
            "开始时间",
            FieldScope::VariantCondition,
            string(""),
        ),
        field("end", "结束时间", FieldScope::VariantCondition, string("")),
        field(
            "weekdays",
            "星期",
            FieldScope::VariantCondition,
            strings(&[]),
        ),
        field(
            "divisor",
            "周期除数",
            FieldScope::VariantCondition,
            FieldValue::Integer(1),
        ),
        field(
            "remainder",
            "周期余数",
            FieldScope::VariantCondition,
            FieldValue::Integer(0),
        ),
        field(
            "timezone",
            "时区/客户端",
            FieldScope::VariantCondition,
            string("Official"),
        ),
        field("stage", "关卡", FieldScope::VariantParam, string("")),
        field(
            "medicine",
            "理智药",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "medicine_expire_days",
            "临期理智药",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "expiring_medicine",
            "旧版过期药数量",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "stone",
            "碎石数量",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "times",
            "战斗次数",
            FieldScope::VariantParam,
            FieldValue::Integer(0),
        ),
        field(
            "series",
            "连战次数",
            FieldScope::VariantParam,
            FieldValue::Integer(1),
        ),
    ]
}

fn validate_field_value(key: &str, value: &FieldValue) -> anyhow::Result<()> {
    match (key, value) {
        ("series", FieldValue::Integer(value)) if !(-1..=6).contains(value) => {
            anyhow::bail!("连战次数必须为 -1 到 6")
        }
        ("threshold", FieldValue::Float(value)) if !(0.0..=1.0).contains(value) => {
            anyhow::bail!("心情阈值必须为 0 到 1")
        }
        ("formation_index", FieldValue::Integer(value)) if !(0..=4).contains(value) => {
            anyhow::bail!("编队编号必须为 0 到 4")
        }
        ("support_unit_usage", FieldValue::Integer(value)) if !(0..=3).contains(value) => {
            anyhow::bail!("助战模式必须为 0 到 3")
        }
        (
            "times"
            | "medicine"
            | "medicine_expire_days"
            | "expiring_medicine"
            | "stone"
            | "loop_times"
            | "divisor"
            | "remainder",
            FieldValue::Integer(value),
        ) if *value < 0 => anyhow::bail!("该数值不能小于 0"),
        ("type", FieldValue::String(value)) if is_compound_condition_type(value) => {
            anyhow::bail!("组合条件暂不支持可视化创建，请保留并手动维护原配置")
        }
        _ => Ok(()),
    }
}

fn field(
    key: &'static str,
    label: &'static str,
    scope: FieldScope,
    default: FieldValue,
) -> FieldSpec {
    FieldSpec {
        key,
        label,
        scope,
        editor: editor_for(key),
        default,
    }
}

fn editor_for(key: &str) -> FieldEditor {
    match key {
        "client_type" | "client" | "timezone" => FieldEditor::Select {
            options: client_options(),
            allow_custom: true,
        },
        "series" => FieldEditor::Select {
            options: series_options(),
            allow_custom: false,
        },
        "mode" => FieldEditor::Select {
            options: infrast_mode_options(),
            allow_custom: false,
        },
        "drones" => FieldEditor::Select {
            options: drone_options(),
            allow_custom: true,
        },
        "medicine_expire_days" => FieldEditor::Select {
            options: medicine_expire_options(),
            allow_custom: false,
        },
        "formation_index" => FieldEditor::Select {
            options: copilot_formation_options(),
            allow_custom: false,
        },
        "support_unit_usage" => FieldEditor::Select {
            options: support_usage_options(),
            allow_custom: false,
        },
        "extra_tags_mode" => FieldEditor::Select {
            options: extra_tags_options(),
            allow_custom: false,
        },
        "server" => FieldEditor::Select {
            options: server_options(),
            allow_custom: false,
        },
        "select" | "confirm" => FieldEditor::MultiSelect {
            options: recruit_star_options(),
            allow_custom: false,
        },
        "facility" => FieldEditor::MultiSelect {
            options: facility_options(),
            allow_custom: true,
        },
        "stage" => FieldEditor::Stage { allow_custom: true },
        "type" => FieldEditor::Select {
            options: condition_type_options(),
            allow_custom: false,
        },
        _ => FieldEditor::Text,
    }
}

fn option(value: FieldValue, label: &str, description: &str) -> SelectOption {
    SelectOption {
        value,
        label: label.to_string(),
        description: description.to_string(),
    }
}

fn client_options() -> Vec<SelectOption> {
    [
        ("", "不指定", "沿用 maa-cli / MaaCore 的默认客户端"),
        ("Official", "官服", "Arknights 官方服务器"),
        ("Bilibili", "Bilibili", "Bilibili 服务器"),
        ("txwy", "森空岛 / txwy", "txwy 客户端"),
        ("YoStarEN", "国际服 EN", "YoStar 英文客户端"),
        ("YoStarJP", "国际服 JP", "YoStar 日文客户端"),
        ("YoStarKR", "国际服 KR", "YoStar 韩文客户端"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn series_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(-1),
        "禁用切换",
        "series=-1：不切换连战倍率",
    ))
    .chain(std::iter::once(option(
        FieldValue::Integer(0),
        "AUTO",
        "series=0：自动使用当前可用的最大连战次数",
    )))
    .chain((1..=6).map(|value| {
        option(
            FieldValue::Integer(value),
            &format!("固定 {value} 次"),
            &format!("series={value}：固定使用 {value} 次连战"),
        )
    }))
    .collect()
}

fn infrast_mode_options() -> Vec<SelectOption> {
    [
        (0, "默认模式", "单设施最优解；按 facility 顺序换班"),
        (10_000, "自定义排班", "读取 filename 指定的 JSON 排班计划"),
        (20_000, "一键轮换", "跳过控制中枢、电站、宿舍和办公室"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

fn drone_options() -> Vec<SelectOption> {
    [
        ("_NotUse", "不使用", "不使用无人机加速"),
        ("Money", "贸易站：龙门币", "用于贸易站获取龙门币"),
        ("SyntheticJade", "贸易站：合成玉", "用于贸易站获取合成玉"),
        ("CombatRecord", "制造站：作战记录", "用于制造站生产作战记录"),
        ("PureGold", "制造站：赤金", "用于制造站生产赤金"),
        ("OriginStone", "制造站：源石", "用于制造站生产源石材料"),
        ("Chip", "制造站：芯片", "用于制造站生产芯片"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn medicine_expire_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(0),
        "不使用临期药",
        "medicine_expire_days=0：不使用即将到期的理智药",
    ))
    .chain((1..=7).map(|days| {
        let hours = days * 24;
        option(
            FieldValue::Integer(days),
            &format!("{hours} 小时内到期"),
            &format!("使用未来 {hours} 小时内到期的理智药；符合条件的药剂会持续使用"),
        )
    }))
    .collect()
}

fn recruit_star_options() -> Vec<SelectOption> {
    (1..=6)
        .rev()
        .map(|star| {
            option(
                FieldValue::Integer(star),
                &format!("{star}★"),
                &format!("允许选择 {star} 星级标签组合"),
            )
        })
        .collect()
}

fn facility_options() -> Vec<SelectOption> {
    [
        ("Mfg", "制造站", "Mfg"),
        ("Trade", "贸易站", "Trade"),
        ("Control", "控制中枢", "Control"),
        ("Power", "发电站", "Power"),
        ("Reception", "会客室", "Reception"),
        ("Office", "办公室", "Office"),
        ("Dorm", "宿舍", "Dorm"),
        ("Processing", "加工站", "Processing"),
        ("Training", "训练室", "Training"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn copilot_formation_options() -> Vec<SelectOption> {
    std::iter::once(option(
        FieldValue::Integer(0),
        "当前编队",
        "formation_index=0：使用当前编队",
    ))
    .chain((1..=4).map(|index| {
        option(
            FieldValue::Integer(index),
            &format!("编队 {index}"),
            &format!("使用作业文件中的第 {index} 个编队"),
        )
    }))
    .collect()
}

fn extra_tags_options() -> Vec<SelectOption> {
    [
        (0, "默认", "默认标签选择策略"),
        (1, "强制三标签", "即使可能冲突也选择三个标签"),
        (2, "优先高星组合", "尽可能选择高星组合，即使可能冲突"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

fn server_options() -> Vec<SelectOption> {
    [
        ("CN", "中国大陆", "向中国大陆统计服务汇报"),
        ("US", "美国", "向美国统计服务汇报"),
        ("JP", "日本", "向日本统计服务汇报"),
        ("KR", "韩国", "向韩国统计服务汇报"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn support_usage_options() -> Vec<SelectOption> {
    [
        (0, "不使用助战", "不使用支援干员"),
        (1, "仅补一名缺失", "仅当恰好缺少一名干员时寻找助战"),
        (
            2,
            "缺失时补齐，否则指定",
            "缺少一名时补齐，否则使用指定助战干员",
        ),
        (
            3,
            "缺失时补齐，否则随机",
            "缺少一名时补齐，否则使用随机助战干员",
        ),
    ]
    .into_iter()
    .map(|(value, label, description)| option(FieldValue::Integer(value), label, description))
    .collect()
}

const EDITABLE_CONDITION_TYPES: [&str; 6] = [
    "Always",
    "Time",
    "DateTime",
    "Weekday",
    "DayMod",
    "OnSideStory",
];

fn is_editable_condition_type(value: &FieldValue) -> bool {
    matches!(value, FieldValue::String(value) if EDITABLE_CONDITION_TYPES.contains(&value.as_str()))
}

fn is_compound_condition_type(value: &str) -> bool {
    matches!(value, "And" | "Or" | "Not")
}

fn condition_type_options() -> Vec<SelectOption> {
    [
        ("Always", "始终", "始终匹配该变体"),
        ("Time", "时间段", "按每日时间段匹配"),
        ("DateTime", "日期时间", "按日期时间范围匹配"),
        ("Weekday", "星期", "按星期匹配"),
        ("DayMod", "日期周期", "按日期除数和余数匹配"),
        ("OnSideStory", "活动开放", "按客户端热更新活动状态匹配"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn stage_options() -> Vec<SelectOption> {
    [
        (
            "",
            "当前/上次作战",
            "留空 stage，沿用游戏当前或最近一次作战入口",
        ),
        ("1-7", "主线 1-7", "固定选择主线 1-7"),
        ("CE-6", "CE-6 龙门币", "龙门币资源关"),
        ("LS-6", "LS-6 作战记录", "作战记录资源关"),
        ("CA-5", "CA-5 技能书", "技能概要资源关"),
        ("AP-5", "AP-5 采购凭证", "采购凭证资源关"),
        ("SK-5", "SK-5 建造材料", "建造材料资源关"),
        ("PR-A-2", "PR-A-2 芯片", "芯片资源关示例"),
    ]
    .into_iter()
    .map(|(value, label, description)| option(string(value), label, description))
    .collect()
}

fn string(value: &str) -> FieldValue {
    FieldValue::String(value.to_string())
}

fn strings(values: &[&str]) -> FieldValue {
    FieldValue::StringArray(values.iter().map(|value| (*value).to_string()).collect())
}

fn integers(values: &[i64]) -> FieldValue {
    FieldValue::IntegerArray(values.to_vec())
}

fn task_type_label(task_type: &str) -> &str {
    match task_type {
        "StartUp" => "启动游戏",
        "Recruit" => "公开招募",
        "Fight" => "刷理智",
        "Infrast" => "基建换班",
        "Mall" => "信用商店",
        "Award" => "领取奖励",
        "CloseDown" => "关闭游戏",
        "Copilot" => "自动战斗",
        _ => task_type,
    }
}

fn log_scope_for_command(command: &TaskCommand) -> LogScope {
    match command.kind {
        TaskKind::Daily => LogScope::Daily,
        TaskKind::Copilot => LogScope::Copilot,
        TaskKind::Update => LogScope::Update,
    }
}

fn is_resource_update_command(command: &TaskCommand) -> bool {
    command.args.first().is_some_and(|arg| arg == "hot-update")
        || command.label.contains("资源热更新")
}

fn looks_like_outdated_resource_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tile-pos")
        || lower.contains("resources may be outdated")
        || lower.contains("unsupportedlevel")
        || text.contains("资源可能过旧")
        || text.contains("资源过旧")
}

fn looks_like_ocr_to_as_t0_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("tile-pos") && lower.contains("t0-")
}

fn hot_update_version_path() -> PathBuf {
    maa_hot_update_dir()
        .unwrap_or_else(|_| PathBuf::from(".local/share/maa/MaaResource"))
        .join("resource/version.json")
}

fn read_hot_update_version() -> Option<HotUpdateVersion> {
    parse_hot_update_version(&fs::read_to_string(hot_update_version_path()).ok()?)
}

fn parse_hot_update_version(raw: &str) -> Option<HotUpdateVersion> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let activity_name = value
        .pointer("/activity/name")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    let last_updated = value
        .get("last_updated")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string();
    if activity_name.is_empty() && last_updated.is_empty() {
        return None;
    }
    Some(HotUpdateVersion {
        activity_name,
        last_updated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::{CopilotEntry, CopilotEntrySource, CopilotOrigin};
    use std::fs;
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

    fn test_copilot_cache_with_set(
        single_count: usize,
        set_count: usize,
    ) -> (PathBuf, CopilotCache) {
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

    #[test]
    fn menu_logs_are_isolated_and_preserved() {
        let mut app = App::new();
        app.screen = Screen::Daily;
        app.push_log(LogLevel::Info, "daily");
        app.screen = Screen::Copilot;
        app.push_log(LogLevel::Info, "copilot");
        app.screen = Screen::Update;
        app.push_log(LogLevel::Info, "update");

        assert_eq!(app.log_buffer(LogScope::Daily).lines.len(), 1);
        assert_eq!(app.log_buffer(LogScope::Copilot).lines.len(), 1);
        assert_eq!(app.log_buffer(LogScope::Update).lines.len(), 1);
        assert_eq!(app.log_buffer(LogScope::Daily).lines[0].text, "daily");
        assert_eq!(app.log_buffer(LogScope::Copilot).lines[0].text, "copilot");
        assert_eq!(app.log_buffer(LogScope::Update).lines[0].text, "update");
    }

    #[test]
    fn background_copilot_import_does_not_override_or_pollute_daily_run() {
        let mut app = App::new();
        for scope in [LogScope::Daily, LogScope::Copilot, LogScope::Update] {
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
    }

    #[test]
    fn config_is_nested_under_daily_navigation() {
        assert_eq!(
            MainMenuItem::ALL.map(MainMenuItem::label),
            ["每日任务", "自动战斗", "更新管理", "退出"]
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
        app.last_failed = true;

        assert!(app.apply_config(|config| config.toggle_task(0).map(|_| ())));
        assert!(!app.last_failed);
        fs::remove_dir_all(dir).unwrap();
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
        let report =
            import_source(path.to_str().unwrap(), ImportKind::Single, &files, |_| {}).unwrap();
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
        let report =
            import_source(path.to_str().unwrap(), ImportKind::Single, &files, |_| {}).unwrap();
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

        assert!(app.log_buffer(LogScope::Update).lines.iter().any(|log| {
            log.level == LogLevel::Warn && log.text.contains("version.json 未变化")
        }));
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
}
