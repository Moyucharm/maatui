//! App 页面导航与运行态快捷键。

use super::*;

impl App {
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
            Screen::Roguelike => self.handle_roguelike_key(key),
            Screen::Update => self.handle_update_key(key),
        }
    }

    pub(crate) fn handle_shortcut_help_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_main_key(&mut self, key: KeyEvent) {
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
                MainMenuItem::Roguelike => {
                    self.roguelike_section_idx = 0;
                    self.clamp_roguelike_selection();
                    self.screen = Screen::Roguelike;
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

    pub(crate) fn handle_daily_key(&mut self, key: KeyEvent) {
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
                if self.reload_config() {
                    self.screen = Screen::Config;
                }
            }
            Some(Action::Activate) => {
                let daily_idx = self.daily_idx;
                match daily_idx {
                    0 => {
                        self.start_command(TaskCommand::daily());
                    }
                    1 if self.reload_config() => self.screen = Screen::Config,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub(crate) fn handle_config_key(&mut self, key: KeyEvent) {
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
            Some(Action::Reload) => {
                self.reload_config();
            }
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

    pub(crate) fn handle_add_task_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_task_edit_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_variant_list_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_variant_edit_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_copilot_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn handle_single_copilot_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn toggle_current_single_difficulty(&mut self) {
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

    pub(crate) fn handle_copilot_list_key(&mut self, key: KeyEvent) {
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

    pub(super) fn save_copilot_settings(&mut self) {
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

    pub(crate) fn handle_copilot_settings_key(&mut self, key: KeyEvent) {
        const ROWS: usize = 9;
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
                8 => {
                    self.confirm = Some(ConfirmDialog {
                        message: "确认清理 MaaCore 干员头像缓存？\n将删除 cache/avatars 中的 PNG；之后首次自动战斗会重新识别干员头像。"
                            .to_string(),
                        target: ConfirmTarget::ClearAvatarCache,
                    });
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub(crate) fn open_current_single_detail(&mut self) {
        let result = self
            .copilot_cache
            .as_ref()
            .context("作业缓存未加载")
            .and_then(CopilotCache::current_single_detail);
        match result {
            Ok(CopilotDetail {
                title,
                overview,
                operators,
                skill_actions,
                groups,
                notes,
            }) => {
                self.copilot_detail = Some(CopilotDetailDialog {
                    title: format!("单作业详情 · {title}"),
                    overview,
                    operators,
                    skill_actions,
                    groups,
                    notes,
                    scroll: 0,
                });
            }
            Err(error) => self.status_error(error.to_string()),
        }
    }

    pub(crate) fn open_selected_copilot_detail(&mut self) {
        let Some(index) = self.selected_copilot_index() else {
            return;
        };
        let result = self
            .copilot_cache
            .as_ref()
            .context("作业列表未加载")
            .and_then(|cache| cache.detail(index));
        match result {
            Ok(CopilotDetail {
                title,
                overview,
                operators,
                skill_actions,
                groups,
                notes,
            }) => {
                debug_assert_eq!(self.copilot_section(), CopilotSection::Sets);
                self.copilot_detail = Some(CopilotDetailDialog {
                    title: format!("作业集条目详情 · {title}"),
                    overview,
                    operators,
                    skill_actions,
                    groups,
                    notes,
                    scroll: 0,
                });
            }
            Err(error) => self.status_error(error.to_string()),
        }
    }

    pub(crate) fn handle_copilot_detail_key(&mut self, key: KeyEvent) {
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

    pub(crate) fn scroll_copilot_detail_up(&mut self, amount: u16) {
        if let Some(detail) = self.copilot_detail.as_mut() {
            detail.scroll = detail.scroll.saturating_sub(amount);
        }
    }

    pub(crate) fn scroll_copilot_detail_down(&mut self, amount: u16) {
        if let Some(detail) = self.copilot_detail.as_mut() {
            detail.scroll = detail.scroll.saturating_add(amount);
        }
    }

    pub(crate) fn handle_update_key(&mut self, key: KeyEvent) {
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
}
