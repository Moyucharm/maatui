//! 应用状态模型与对话框数据。

use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use crate::config::{DailyConfig, FieldValue, TaskSummary};
use crate::copilot::{CopilotCache, CopilotOptions, ImportKind, ImportProgress, ImportReport};
use crate::copilot_run::CopilotBatchState;
use crate::runner::{LogLevel, RunningTask, TaskCommand};
use crate::stage::{StageCatalog, StageRefreshEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProgress {
    pub current: usize,
    pub total: usize,
    pub label: String,
}

#[derive(Debug, Clone)]
pub(super) struct DailyRunState {
    pub(super) tasks: Vec<TaskSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainMenuItem {
    Daily,
    Copilot,
    Update,
    Quit,
}

impl MainMenuItem {
    pub const ALL: [Self; 4] = [Self::Daily, Self::Copilot, Self::Update, Self::Quit];

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "每日任务",
            Self::Copilot => "自动战斗",
            Self::Update => "更新管理",
            Self::Quit => "退出",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Daily => "运行与配置 daily 任务",
            Self::Copilot => "单作业 / 作业集（批量）",
            Self::Update => "手动更新资源或 Core",
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
    Update,
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
pub enum LogScope {
    Daily,
    Copilot,
    Update,
}

#[derive(Debug, Clone)]
pub struct LogBuffer {
    pub lines: VecDeque<LogLine>,
    pub scroll: u16,
    pub auto_scroll: bool,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            scroll: 0,
            auto_scroll: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MenuLogs {
    pub daily: LogBuffer,
    pub copilot: LogBuffer,
    pub update: LogBuffer,
}

impl MenuLogs {
    pub fn get(&self, scope: LogScope) -> &LogBuffer {
        match scope {
            LogScope::Daily => &self.daily,
            LogScope::Copilot => &self.copilot,
            LogScope::Update => &self.update,
        }
    }

    pub fn get_mut(&mut self, scope: LogScope) -> &mut LogBuffer {
        match scope {
            LogScope::Daily => &mut self.daily,
            LogScope::Copilot => &mut self.copilot,
            LogScope::Update => &mut self.update,
        }
    }
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

impl FieldSpec {
    pub fn display_value(&self, value: &FieldValue) -> String {
        match &self.editor {
            FieldEditor::Select { options, .. } => {
                option_label(options, value).unwrap_or_else(|| value.display())
            }
            FieldEditor::MultiSelect { options, .. } => match value {
                FieldValue::IntegerArray(values) => values
                    .iter()
                    .map(|value| {
                        let value = FieldValue::Integer(*value);
                        option_label(options, &value).unwrap_or_else(|| value.display())
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                FieldValue::StringArray(values) => values
                    .iter()
                    .map(|value| {
                        let value = FieldValue::String(value.clone());
                        option_label(options, &value).unwrap_or_else(|| value.display())
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => value.display(),
            },
            _ => value.display(),
        }
    }
}

pub(super) fn option_label(options: &[SelectOption], value: &FieldValue) -> Option<String> {
    options
        .iter()
        .find(|option| &option.value == value)
        .map(|option| option.label.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotSection {
    Singles,
    Sets,
    Settings,
}

impl CopilotSection {
    pub const ALL: [Self; 3] = [Self::Singles, Self::Sets, Self::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Self::Singles => "单作业",
            Self::Sets => "作业集（批量）",
            Self::Settings => "运行设置",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportDestination {
    CurrentSingle,
    Batch,
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
    CopilotAdd {
        kind: ImportKind,
        destination: ImportDestination,
    },
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

impl InputDialog {
    pub fn allows_import_kind_switch(&self) -> bool {
        matches!(
            self.target,
            InputTarget::CopilotAdd {
                destination: ImportDestination::Batch,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SelectBehavior {
    pub(super) allow_custom: bool,
    pub(super) refreshable: bool,
    pub(super) multi: bool,
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

#[derive(Debug, Clone)]
pub enum ConfirmTarget {
    DeleteTask(usize),
    DeleteVariant { task: usize, variant: usize },
    DeleteCopilot(usize),
    ClearCopilotEntries,
    RunCommand(TaskCommand),
}

#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub message: String,
    pub target: ConfirmTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotUpdateVersion {
    pub activity_name: String,
    pub last_updated: String,
}

impl HotUpdateVersion {
    pub fn summary(&self) -> String {
        match (self.activity_name.is_empty(), self.last_updated.is_empty()) {
            (false, false) => format!(
                "热更新资源 · {} · {}",
                self.activity_name, self.last_updated
            ),
            (false, true) => format!("热更新资源 · {}", self.activity_name),
            (true, false) => format!("热更新资源 · 更新于 {}", self.last_updated),
            (true, true) => "热更新资源 · 版本信息缺失".to_string(),
        }
    }
}

pub(super) enum CopilotImportEvent {
    Progress(ImportProgress),
    Finished {
        destination: ImportDestination,
        result: Result<ImportReport, String>,
    },
}

#[derive(Debug, Clone)]
pub struct CopilotDetailDialog {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: u16,
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
    pub copilot_settings_idx: usize,
    pub copilot_section_idx: usize,
    pub update_idx: usize,
    pub phase: TaskPhase,
    pub menu_logs: MenuLogs,
    pub active_log_scope: LogScope,
    pub status_text: String,
    pub should_quit: bool,
    pub started_at: Option<Instant>,
    pub active_label: String,
    pub last_failed: bool,
    pub config: Option<DailyConfig>,
    pub config_error: Option<String>,
    pub copilot: CopilotOptions,
    pub copilot_cache: Option<CopilotCache>,
    pub copilot_error: Option<String>,
    pub current_single_supported_modes: Option<(bool, bool)>,
    pub copilot_importing: bool,
    pub copilot_import_progress: Option<ImportProgress>,
    pub stage_catalog: StageCatalog,
    pub input: Option<InputDialog>,
    pub select: Option<SelectDialog>,
    pub confirm: Option<ConfirmDialog>,
    pub copilot_detail: Option<CopilotDetailDialog>,
    pub shortcut_help_open: bool,
    pub shortcut_help_scroll: u16,
    pub run_progress: Option<RunProgress>,
    pub(super) task: Option<RunningTask>,
    pub(super) daily_run: Option<DailyRunState>,
    pub(super) pending_resource_check: bool,
    pub(super) resource_version_before: Option<HotUpdateVersion>,
    pub(super) saw_outdated_resource_error: bool,
    pub(super) saw_ocr_to_t0_error: bool,
    pub(super) copilot_batch: Option<CopilotBatchState>,
    pub(super) copilot_import_tx: std::sync::mpsc::Sender<CopilotImportEvent>,
    pub(super) copilot_import_rx: Receiver<CopilotImportEvent>,
    pub(super) stage_refresh_tx: std::sync::mpsc::Sender<StageRefreshEvent>,
    pub(super) stage_refresh_rx: Receiver<StageRefreshEvent>,
    pub(super) stage_refreshing: bool,
    pub(super) animation_started: Instant,
}
