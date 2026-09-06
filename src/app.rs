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
use crate::config_save;
use crate::copilot::{
    BatchTask, CopilotCache, CopilotDetail, CopilotRunOptions, ImportKind, ImportReport,
    import_source, remove_batch_task, write_batch_task,
};
use crate::copilot_run::{BatchPosition, CopilotBatchState};
use crate::notification::{self, NotificationWorker, TaskOutcome};
use crate::roguelike as roguelike_model;
use crate::runner::{LogLevel, RunnerEvent, RunningTask, TaskCommand, TaskKind};
use crate::shortcuts::{Action, ShortcutContext, action_for};
use crate::stage::{StageCatalog, StageRefreshEvent};
use crate::storage::clear_maa_avatar_cache;
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

mod copilot;
mod daily;
mod input;
mod navigation;
mod roguelike;
mod schema;
mod state;
mod task_lifecycle;
#[cfg(test)]
mod tests;

#[cfg(test)]
use schema::is_compound_condition_type;
#[cfg(test)]
pub(crate) use schema::{condition_type_options, parse_hot_update_version, series_options, string};
use schema::{
    copilot_formation_options, is_editable_condition_type, looks_like_ocr_to_as_t0_error,
    looks_like_outdated_resource_error, read_hot_update_version, stage_options,
    support_usage_options, task_type_label, validate_field_value,
};
pub(crate) use schema::{task_fields, variant_fields};
pub use state::*;
use state::{CopilotImportEvent, DailyRunState, SelectBehavior, option_label};
#[cfg(test)]
pub(crate) use task_lifecycle::fully_disabled_chains;
#[cfg(test)]
pub(crate) use task_lifecycle::log_scope_for_command;

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
        let (roguelike_config, roguelike_error) =
            match roguelike_model::RoguelikeConfig::load_default() {
                Ok(config) => (Some(config), None),
                Err(error) => (None, Some(error.to_string())),
            };
        let roguelike = roguelike_config
            .as_ref()
            .map(|config| config.options().clone())
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
            roguelike_idx: 0,
            roguelike_advanced_idx: 0,
            roguelike_section_idx: 0,
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
            config_save_worker: if cfg!(test) {
                None
            } else {
                config_save::ConfigSaveWorker::new().ok()
            },
            config_revision: 0,
            config_dirty: false,
            config_enqueued_revision: None,
            config_failed_revision: None,
            copilot,
            copilot_cache,
            copilot_error,
            roguelike,
            roguelike_config,
            roguelike_error,
            current_single_supported_modes,
            copilot_importing: false,
            copilot_import_progress: None,
            stage_catalog,
            stage_options_cache: std::cell::RefCell::new(None),
            input: None,
            select: None,
            confirm: None,
            copilot_detail: None,
            shortcut_help_open: false,
            shortcut_help_scroll: 0,
            run_progress: None,
            temp_task_file: None,
            task: None,
            active_task_kind: None,
            task_abort_error: None,
            notification_worker: NotificationWorker::new().ok(),
            daily_run: None,
            disabled_task_chains: Vec::new(),
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
        if let Some(config) = app.config.as_mut() {
            config.set_defer_save(true);
        }
        if migrated_single_reset {
            let message =
                "缓存已升级：旧单作业列表已清空，请重新搜索当前单作业；作业集列表保持不变";
            app.push_log_to(LogScope::Copilot, LogLevel::Warn, message);
        }
        if let Some(error) = app.roguelike_error.clone() {
            let message = format!("自动肉鸽配置加载失败: {error}");
            app.status_text = message.clone();
            app.push_log_to(LogScope::Roguelike, LogLevel::Error, message);
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

    pub fn roguelike_section(&self) -> roguelike_model::RoguelikeSection {
        roguelike_model::RoguelikeSection::ALL[self
            .roguelike_section_idx
            .min(roguelike_model::RoguelikeSection::ALL.len() - 1)]
    }

    pub fn shortcut_context(&self) -> ShortcutContext {
        ShortcutContext::from_app(
            self.phase,
            self.screen,
            self.editor_section(),
            self.copilot_section(),
            self.roguelike_section(),
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
            Screen::Roguelike => LogScope::Roguelike,
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

    fn reload_config(&mut self) -> bool {
        self.reload_config_with(DailyConfig::load_default)
    }

    #[cfg(test)]
    pub(super) fn reload_config_from(&mut self, path: PathBuf) -> bool {
        self.reload_config_with(|| DailyConfig::load(&path))
    }

    fn reload_config_with<F>(&mut self, load: F) -> bool
    where
        F: FnOnce() -> anyhow::Result<DailyConfig>,
    {
        if self.flush_config_save().is_err() {
            return false;
        }
        match load() {
            Ok(mut config) => {
                config.set_defer_save(true);
                self.config = Some(config);
                self.config_error = None;
                self.stage_options_cache.borrow_mut().take();
                self.last_failed = false;
                let len = self.config.as_ref().map_or(0, DailyConfig::len);
                self.config_idx = self.config_idx.min(len.saturating_sub(1));
                true
            }
            Err(error) => {
                self.config = None;
                self.config_error = Some(error.to_string());
                self.status_error(error.to_string());
                false
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
                | (LogScope::Roguelike, Screen::Roguelike)
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
