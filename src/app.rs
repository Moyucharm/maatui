//! 应用状态、分层菜单、配置编辑与任务状态迁移。

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::config::{DailyConfig, FieldValue};
use crate::runner::{LogLevel, RunnerEvent, RunningTask, TaskCommand};
use crate::stage::{StageCatalog, StageRefreshEvent};

const MAX_LOG_LINES: usize = 3000;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainMenuItem {
    Daily,
    Config,
    Copilot,
    Quit,
}

impl MainMenuItem {
    pub const ALL: [Self; 4] = [Self::Daily, Self::Config, Self::Copilot, Self::Quit];

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "每日任务",
            Self::Config => "配置管理",
            Self::Copilot => "自动战斗",
            Self::Quit => "退出",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Daily => "maa run daily -v",
            Self::Config => "编辑 daily 任务项",
            Self::Copilot => "maa copilot <作业> -v",
            Self::Quit => "安全退出 MaaTUI",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    Daily,
    Config,
    AddTask,
    TaskEdit,
    VariantList,
    VariantEdit,
    Copilot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSection {
    Basic,
    Advanced,
    Variants,
}

impl EditorSection {
    pub const ALL: [Self; 3] = [Self::Basic, Self::Advanced, Self::Variants];

    pub fn label(self) -> &'static str {
        match self {
            Self::Basic => "基础设置",
            Self::Advanced => "高级设置",
            Self::Variants => "条件与变体",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    Idle,
    Running,
    Stopping,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldScope {
    Task,
    Param,
    VariantParam,
    VariantCondition,
}

#[derive(Debug, Clone)]
pub struct SelectOption {
    pub value: FieldValue,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub enum FieldEditor {
    Text,
    Select {
        options: Vec<SelectOption>,
        allow_custom: bool,
    },
    MultiSelect {
        options: Vec<SelectOption>,
        allow_custom: bool,
    },
    Stage {
        allow_custom: bool,
    },
}

#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub scope: FieldScope,
    pub default: FieldValue,
    pub editor: FieldEditor,
}

#[derive(Debug, Clone)]
pub struct CopilotOptions {
    pub source: String,
    pub raid: RaidMode,
    pub formation: bool,
    pub formation_index: i64,
    pub use_sanity_potion: bool,
    pub add_trust: bool,
    pub ignore_requirements: bool,
    pub support_unit_usage: i64,
    pub support_unit_name: String,
    pub loop_times: i64,
}

impl Default for CopilotOptions {
    fn default() -> Self {
        Self {
            source: String::new(),
            raid: RaidMode::Normal,
            formation: false,
            formation_index: 0,
            use_sanity_potion: false,
            add_trust: false,
            ignore_requirements: false,
            support_unit_usage: 0,
            support_unit_name: String::new(),
            loop_times: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidMode {
    Normal,
    Raid,
    Both,
}

impl RaidMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "普通",
            Self::Raid => "突袭",
            Self::Both => "普通 + 突袭",
        }
    }

    fn cli_value(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Raid => "raid",
            Self::Both => "both",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Normal => Self::Raid,
            Self::Raid => Self::Both,
            Self::Both => Self::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub enum CopilotSource {
    SingleCode(u64),
    SetCode(u64),
    Local(String),
}

impl CopilotSource {
    fn cli_arg(&self) -> String {
        match self {
            Self::SingleCode(code) => format!("maa://{code}"),
            Self::SetCode(code) => format!("maa://{code}s"),
            Self::Local(path) => path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum InputTarget {
    TaskField {
        task: usize,
        field: FieldSpec,
    },
    VariantField {
        task: usize,
        variant: usize,
        field: FieldSpec,
    },
    CopilotSource,
    CopilotNumber(CopilotNumberField),
    CopilotText(CopilotTextField),
}

#[derive(Debug, Clone, Copy)]
pub enum CopilotNumberField {
    FormationIndex,
    SupportUsage,
    LoopTimes,
}

#[derive(Debug, Clone, Copy)]
pub enum CopilotTextField {
    SupportName,
}

#[derive(Debug, Clone)]
pub struct InputDialog {
    pub title: String,
    pub value: String,
    pub target: InputTarget,
}

#[derive(Debug, Clone, Copy)]
struct SelectBehavior {
    allow_custom: bool,
    refreshable: bool,
    multi: bool,
}

#[derive(Debug, Clone)]
pub struct SelectDialog {
    pub title: String,
    pub options: Vec<SelectOption>,
    pub selected: usize,
    pub allow_custom: bool,
    pub refreshable: bool,
    pub multi: bool,
    pub selected_values: Vec<FieldValue>,
    pub current: FieldValue,
    pub target: InputTarget,
}

#[derive(Debug, Clone, Copy)]
pub enum ConfirmTarget {
    DeleteTask(usize),
    DeleteVariant { task: usize, variant: usize },
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub message: String,
    pub target: ConfirmTarget,
}

pub struct App {
    pub screen: Screen,
    pub main_idx: usize,
    pub daily_idx: usize,
    pub config_idx: usize,
    pub add_task_idx: usize,
    pub section_idx: usize,
    pub field_idx: usize,
    pub variant_idx: usize,
    pub copilot_idx: usize,
    pub phase: TaskPhase,
    pub logs: VecDeque<LogLine>,
    pub scroll: u16,
    pub auto_scroll: bool,
    pub status_text: String,
    pub should_quit: bool,
    pub started_at: Option<Instant>,
    pub active_label: String,
    pub last_failed: bool,
    pub config: Option<DailyConfig>,
    pub config_error: Option<String>,
    pub copilot: CopilotOptions,
    pub stage_catalog: StageCatalog,
    pub input: Option<InputDialog>,
    pub select: Option<SelectDialog>,
    pub confirm: Option<ConfirmDialog>,
    task: Option<RunningTask>,
    stage_refresh_tx: std::sync::mpsc::Sender<StageRefreshEvent>,
    stage_refresh_rx: Receiver<StageRefreshEvent>,
    animation_started: Instant,
}

impl App {
    pub fn new() -> Self {
        let (config, config_error) = match DailyConfig::load_default() {
            Ok(config) => (Some(config), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let (stage_refresh_tx, stage_refresh_rx) = mpsc::channel();
        let stage_catalog = StageCatalog::load_cached();
        StageCatalog::spawn_refresh(stage_refresh_tx.clone());
        Self {
            screen: Screen::Main,
            main_idx: 0,
            daily_idx: 0,
            config_idx: 0,
            add_task_idx: 0,
            section_idx: 0,
            field_idx: 0,
            variant_idx: 0,
            copilot_idx: 0,
            phase: TaskPhase::Idle,
            logs: VecDeque::new(),
            scroll: 0,
            auto_scroll: true,
            status_text: "就绪".to_string(),
            should_quit: false,
            started_at: None,
            active_label: String::new(),
            last_failed: false,
            config,
            config_error,
            copilot: CopilotOptions::default(),
            stage_catalog,
            input: None,
            select: None,
            confirm: None,
            task: None,
            stage_refresh_tx,
            stage_refresh_rx,
            animation_started: Instant::now(),
        }
    }

    pub fn selected_main(&self) -> MainMenuItem {
        MainMenuItem::ALL[self.main_idx.min(MainMenuItem::ALL.len() - 1)]
    }

    pub fn editor_section(&self) -> EditorSection {
        EditorSection::ALL[self.section_idx.min(EditorSection::ALL.len() - 1)]
    }

    pub fn push_log(&mut self, level: LogLevel, text: impl Into<String>) {
        self.logs.push_back(LogLine {
            level,
            text: text.into(),
        });
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
        if self.auto_scroll {
            self.scroll = self.max_scroll(0);
        }
    }

    pub fn max_scroll(&self, visible_rows: u16) -> u16 {
        (self.logs.len() as u16).saturating_sub(visible_rows.max(1))
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
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

        if self.phase != TaskPhase::Idle {
            match key.code {
                KeyCode::Char('s') | KeyCode::Enter => self.request_stop(),
                KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
                KeyCode::PageUp => self.scroll_logs_up(10),
                KeyCode::PageDown => self.scroll_logs_down(10),
                KeyCode::Home => {
                    self.auto_scroll = false;
                    self.scroll = 0;
                }
                KeyCode::End => {
                    self.auto_scroll = true;
                    self.scroll = u16::MAX;
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
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_logs_up(3),
            MouseEventKind::ScrollDown => self.scroll_logs_down(3),
            _ => {}
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Up | KeyCode::Char('k') => self.main_idx = self.main_idx.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.main_idx = (self.main_idx + 1).min(MainMenuItem::ALL.len() - 1);
            }
            KeyCode::Enter => match self.selected_main() {
                MainMenuItem::Daily => {
                    self.daily_idx = 0;
                    self.screen = Screen::Daily;
                }
                MainMenuItem::Config => {
                    self.reload_config();
                    self.screen = Screen::Config;
                }
                MainMenuItem::Copilot => self.screen = Screen::Copilot,
                MainMenuItem::Quit => self.request_quit(),
            },
            _ => {}
        }
    }

    fn handle_daily_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Main,
            KeyCode::Up | KeyCode::Char('k') => {
                self.daily_idx = self.daily_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.daily_idx = (self.daily_idx + 1).min(1);
            }
            KeyCode::Char('r') => self.start_command(TaskCommand::daily()),
            KeyCode::Char('c') => {
                self.reload_config();
                self.screen = Screen::Config;
            }
            KeyCode::Enter => match self.daily_idx {
                0 => self.start_command(TaskCommand::daily()),
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
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Main,
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.config_idx > 0 {
                    let from = self.config_idx;
                    if self.apply_config(|config| config.move_task(from, from - 1)) {
                        self.config_idx -= 1;
                    }
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.config_idx + 1 < len {
                    let from = self.config_idx;
                    if self.apply_config(|config| config.move_task(from, from + 1)) {
                        self.config_idx += 1;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.config_idx = self.config_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    self.config_idx = (self.config_idx + 1).min(len - 1);
                }
            }
            KeyCode::Char(' ') if len > 0 => {
                let index = self.config_idx;
                self.apply_config(|config| config.toggle_task(index).map(|_| ()));
            }
            KeyCode::Char('a') => {
                self.add_task_idx = 0;
                self.screen = Screen::AddTask;
            }
            KeyCode::Char('d') if len > 0 => {
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
            KeyCode::Enter | KeyCode::Char('e') if len > 0 => {
                self.section_idx = 0;
                self.field_idx = 0;
                self.screen = Screen::TaskEdit;
            }
            KeyCode::Char('r') => self.reload_config(),
            _ => {}
        }
    }

    fn handle_add_task_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Config,
            KeyCode::Up | KeyCode::Char('k') => {
                self.add_task_idx = self.add_task_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.add_task_idx = (self.add_task_idx + 1).min(TASK_TYPES.len() - 1);
            }
            KeyCode::Enter => {
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
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.field_idx = 0;
                self.screen = Screen::Config;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.section_idx = self.section_idx.saturating_sub(1);
                self.field_idx = 0;
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.section_idx = (self.section_idx + 1).min(EditorSection::ALL.len() - 1);
                self.field_idx = 0;
            }
            KeyCode::Enter if self.editor_section() == EditorSection::Variants => {
                self.variant_idx = 0;
                self.screen = Screen::VariantList;
            }
            KeyCode::Up | KeyCode::Char('k') => self.field_idx = self.field_idx.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                let len = self.current_task_fields().len();
                if len > 0 {
                    self.field_idx = (self.field_idx + 1).min(len - 1);
                }
            }
            KeyCode::Char('v') => self.create_stage_variant(),
            KeyCode::Enter | KeyCode::Char('e') => self.edit_current_task_field(),
            _ => {}
        }
    }

    fn handle_variant_list_key(&mut self, key: KeyEvent) {
        let count = self
            .config
            .as_ref()
            .map_or(0, |config| config.variant_count(self.config_idx));
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::TaskEdit,
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.variant_idx > 0 {
                    let task = self.config_idx;
                    let from = self.variant_idx;
                    if self.apply_config(|config| config.move_variant(task, from, from - 1)) {
                        self.variant_idx -= 1;
                    }
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.variant_idx + 1 < count {
                    let task = self.config_idx;
                    let from = self.variant_idx;
                    if self.apply_config(|config| config.move_variant(task, from, from + 1)) {
                        self.variant_idx += 1;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.variant_idx = self.variant_idx.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if count > 0 {
                    self.variant_idx = (self.variant_idx + 1).min(count - 1);
                }
            }
            KeyCode::Char('a') => {
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
            KeyCode::Char('d') if count > 0 => {
                self.confirm = Some(ConfirmDialog {
                    message: "确认删除当前变体？此操作会立即保存。".to_string(),
                    target: ConfirmTarget::DeleteVariant {
                        task: self.config_idx,
                        variant: self.variant_idx,
                    },
                });
            }
            KeyCode::Enter | KeyCode::Char('e') if count > 0 => {
                self.field_idx = 0;
                self.screen = Screen::VariantEdit;
            }
            _ => {}
        }
    }

    fn handle_variant_edit_key(&mut self, key: KeyEvent) {
        let fields = variant_fields();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::VariantList,
            KeyCode::Up | KeyCode::Char('k') => self.field_idx = self.field_idx.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.field_idx = (self.field_idx + 1).min(fields.len() - 1);
            }
            KeyCode::Enter | KeyCode::Char('e') => {
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
        const ROWS: usize = 11;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.screen = Screen::Main,
            KeyCode::Up | KeyCode::Char('k') => {
                self.copilot_idx = self.copilot_idx.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.copilot_idx = (self.copilot_idx + 1).min(ROWS - 1);
            }
            KeyCode::Char('r') => self.start_copilot(),
            KeyCode::Enter | KeyCode::Char('e') => match self.copilot_idx {
                0 => self.open_input(
                    "作业代码 / URI / 本地 JSON".to_string(),
                    self.copilot.source.clone(),
                    InputTarget::CopilotSource,
                ),
                1 => self.copilot.raid = self.copilot.raid.next(),
                2 => self.copilot.formation = !self.copilot.formation,
                3 => self.open_select(
                    "编队编号".to_string(),
                    FieldValue::Integer(self.copilot.formation_index),
                    copilot_formation_options(),
                    false,
                    InputTarget::CopilotNumber(CopilotNumberField::FormationIndex),
                ),
                4 => self.copilot.use_sanity_potion = !self.copilot.use_sanity_potion,
                5 => self.copilot.add_trust = !self.copilot.add_trust,
                6 => self.copilot.ignore_requirements = !self.copilot.ignore_requirements,
                7 => self.open_select(
                    "助战模式".to_string(),
                    FieldValue::Integer(self.copilot.support_unit_usage),
                    support_usage_options(),
                    false,
                    InputTarget::CopilotNumber(CopilotNumberField::SupportUsage),
                ),
                8 => self.open_input(
                    "指定助战干员".to_string(),
                    self.copilot.support_unit_name.clone(),
                    InputTarget::CopilotText(CopilotTextField::SupportName),
                ),
                9 => self.open_input(
                    "循环次数（大于 0）".to_string(),
                    self.copilot.loop_times.to_string(),
                    InputTarget::CopilotNumber(CopilotNumberField::LoopTimes),
                ),
                10 => self.start_copilot(),
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => self.commit_input(),
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
            InputTarget::CopilotSource | InputTarget::CopilotText(_) => {
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
            self.status_text = "已创建 OnSideStory 活动变体".to_string();
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
            InputTarget::CopilotSource => {
                self.copilot.source = dialog.value.trim().to_string();
                Ok(())
            }
            InputTarget::CopilotNumber(field) => dialog
                .value
                .trim()
                .parse::<i64>()
                .context("请输入有效整数")
                .and_then(|value| self.set_copilot_number(field, value)),
            InputTarget::CopilotText(CopilotTextField::SupportName) => {
                self.copilot.support_unit_name = dialog.value.trim().to_string();
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
        StageCatalog::spawn_refresh(self.stage_refresh_tx.clone());
        if self.can_report_stage_refresh_status() {
            self.status_text = "正在刷新活动关卡目录…".to_string();
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
        self.status_text = "配置已自动保存".to_string();
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
        self.status_text = "变体已自动保存".to_string();
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
                self.status_text = "配置已自动保存".to_string();
                self.last_failed = false;
                true
            }
            Err(error) => {
                self.status_error(error.to_string());
                false
            }
        }
    }

    fn reload_config(&mut self) {
        match DailyConfig::load_default() {
            Ok(config) => {
                let path = config.path().display().to_string();
                self.config = Some(config);
                self.config_error = None;
                self.status_text = format!("已加载 {path}");
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

    fn start_copilot(&mut self) {
        let source = match parse_copilot_source(&self.copilot.source) {
            Ok(CopilotSource::SetCode(_)) => {
                self.status_error("已识别作业集，但首版暂未开放作业集执行".to_string());
                return;
            }
            Ok(source) => source,
            Err(error) => {
                self.status_error(error);
                return;
            }
        };

        let mut args = vec![
            "copilot".to_string(),
            source.cli_arg(),
            "--raid".to_string(),
            self.copilot.raid.cli_value().to_string(),
        ];
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
            "-v".to_string(),
        ]);
        self.start_command(TaskCommand::copilot(args));
    }

    fn start_command(&mut self, command: TaskCommand) {
        self.logs.clear();
        self.scroll = 0;
        self.auto_scroll = true;
        self.last_failed = false;
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
            }
            Err(error) => {
                self.push_log(LogLevel::Error, error);
                self.status_text = "启动失败".to_string();
                self.last_failed = true;
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
        let events = self
            .task
            .as_mut()
            .map_or_else(Vec::new, RunningTask::poll_events);
        for event in events {
            match event {
                RunnerEvent::Line { level, text } => self.push_log(level, text),
                RunnerEvent::Exited { code, stopped } => self.on_exited(code, stopped),
            }
        }
    }

    fn poll_stage_refresh(&mut self) {
        while let Ok(event) = self.stage_refresh_rx.try_recv() {
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

    fn on_exited(&mut self, code: Option<i32>, stopped: bool) {
        let code_text = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        if stopped {
            self.push_log(
                LogLevel::Warn,
                format!("{}已停止 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已停止 · exit {code_text}");
            self.last_failed = false;
        } else if code == Some(0) {
            self.push_log(
                LogLevel::Success,
                format!("{}完成 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已完成 · exit {code_text}");
            self.last_failed = false;
        } else {
            self.push_log(
                LogLevel::Error,
                format!("{}异常结束 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("失败 · exit {code_text}");
            self.last_failed = true;
        }
        self.task = None;
        self.phase = TaskPhase::Idle;
        self.started_at = None;
        self.active_label.clear();
        self.auto_scroll = true;
    }

    pub fn cleanup(&mut self) {
        if let Some(task) = self.task.take() {
            task.force_cleanup();
        }
        self.phase = TaskPhase::Idle;
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = (self.animation_started.elapsed().as_millis() / 80) as usize;
        FRAMES[frame % FRAMES.len()]
    }

    fn scroll_logs_up(&mut self, amount: u16) {
        self.auto_scroll = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn scroll_logs_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    fn status_error(&mut self, message: String) {
        self.status_text = format!("错误: {message}");
        self.last_failed = true;
        self.push_log(LogLevel::Error, message);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_copilot_source(input: &str) -> Result<CopilotSource, String> {
    let value = input.trim();
    if value.is_empty() {
        return Err("请输入作业代码、maa:// URI 或本地 JSON 路径".to_string());
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return value
            .parse()
            .map(CopilotSource::SingleCode)
            .map_err(|_| "作业代码无效".to_string());
    }
    for prefix in ["maa://", "prts://"] {
        if let Some(code) = value.strip_prefix(prefix) {
            if let Some(code) = code.strip_suffix('s') {
                return code
                    .parse()
                    .map(CopilotSource::SetCode)
                    .map_err(|_| "作业集代码无效".to_string());
            }
            return code
                .parse()
                .map(CopilotSource::SingleCode)
                .map_err(|_| "作业代码无效".to_string());
        }
    }
    if value.starts_with("file://") || Path::new(value).extension().is_some() {
        return Ok(CopilotSource::Local(value.to_string()));
    }
    Err("无法识别作业来源；支持纯数字、maa://、prts:// 和本地 JSON".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn parses_supported_copilot_sources() {
        assert!(matches!(
            parse_copilot_source("123"),
            Ok(CopilotSource::SingleCode(123))
        ));
        assert!(matches!(
            parse_copilot_source("maa://456"),
            Ok(CopilotSource::SingleCode(456))
        ));
        assert!(matches!(
            parse_copilot_source("prts://789"),
            Ok(CopilotSource::SingleCode(789))
        ));
        assert!(matches!(
            parse_copilot_source("maa://42s"),
            Ok(CopilotSource::SetCode(42))
        ));
        assert!(matches!(
            parse_copilot_source("./plan.json"),
            Ok(CopilotSource::Local(_))
        ));
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
    }
}
