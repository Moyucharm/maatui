//! 页面快捷键动作、匹配规则与帮助元数据。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{CopilotSection, EditorSection, Screen, TaskPhase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutContext {
    Running,
    Main,
    Daily,
    Config,
    AddTask,
    TaskEdit,
    TaskVariants,
    VariantList,
    VariantEdit,
    CopilotSingles,
    CopilotSets,
    CopilotSettings,
    Update,
}

impl ShortcutContext {
    pub fn from_app(
        phase: TaskPhase,
        screen: Screen,
        editor_section: EditorSection,
        copilot_section: CopilotSection,
    ) -> Self {
        if phase != TaskPhase::Idle {
            return Self::Running;
        }
        match screen {
            Screen::Main => Self::Main,
            Screen::Daily => Self::Daily,
            Screen::Config => Self::Config,
            Screen::AddTask => Self::AddTask,
            Screen::TaskEdit if editor_section == EditorSection::Variants => Self::TaskVariants,
            Screen::TaskEdit => Self::TaskEdit,
            Screen::VariantList => Self::VariantList,
            Screen::VariantEdit => Self::VariantEdit,
            Screen::Copilot => match copilot_section {
                CopilotSection::Singles => Self::CopilotSingles,
                CopilotSection::Sets => Self::CopilotSets,
                CopilotSection::Settings => Self::CopilotSettings,
            },
            Screen::Update => Self::Update,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Back,
    Previous,
    Next,
    MoveUp,
    MoveDown,
    Activate,
    Edit,
    Toggle,
    Add,
    Delete,
    Reload,
    RunDaily,
    RunDailySingle,
    OpenConfig,
    PreviousEditorSection,
    NextEditorSection,
    CreateVariant,
    PreviousCopilotTab,
    NextCopilotTab,
    SearchSingle,
    ToggleDifficulty,
    ShowDetail,
    RunSingle,
    RunSelected,
    RunBatch,
    ToggleAll,
    Clear,
    Stop,
    QuitRunning,
    LogPageUp,
    LogPageDown,
    LogHome,
    LogEnd,
}

impl Action {
    fn matches(self, key: KeyEvent) -> bool {
        match self {
            Self::Back => matches!(key.code, KeyCode::Esc | KeyCode::Char('q')),
            Self::Previous => matches!(key.code, KeyCode::Up | KeyCode::Char('k')),
            Self::Next => matches!(key.code, KeyCode::Down | KeyCode::Char('j')),
            Self::MoveUp => key.code == KeyCode::Up && key.modifiers.contains(KeyModifiers::SHIFT),
            Self::MoveDown => {
                key.code == KeyCode::Down && key.modifiers.contains(KeyModifiers::SHIFT)
            }
            Self::Activate => key.code == KeyCode::Enter,
            Self::Edit => matches!(key.code, KeyCode::Enter | KeyCode::Char('e')),
            Self::Toggle => key.code == KeyCode::Char(' '),
            Self::Add => key.code == KeyCode::Char('a'),
            Self::Delete => key.code == KeyCode::Char('d'),
            Self::Reload => key.code == KeyCode::Char('r'),
            Self::RunDaily => key.code == KeyCode::Char('r'),
            Self::RunDailySingle => key.code == KeyCode::Char('x'),
            Self::OpenConfig => key.code == KeyCode::Char('c'),
            Self::PreviousEditorSection => {
                matches!(key.code, KeyCode::Left | KeyCode::Char('h'))
            }
            Self::NextEditorSection => {
                matches!(key.code, KeyCode::Right | KeyCode::Char('l'))
            }
            Self::CreateVariant => key.code == KeyCode::Char('v'),
            Self::PreviousCopilotTab => key.code == KeyCode::Left,
            Self::NextCopilotTab => matches!(key.code, KeyCode::Tab | KeyCode::Right),
            Self::SearchSingle => matches!(key.code, KeyCode::Char('a') | KeyCode::Char('e')),
            Self::ToggleDifficulty => key.code == KeyCode::Char(' '),
            Self::ShowDetail => key.code == KeyCode::Char('i'),
            Self::RunSingle => key.code == KeyCode::Enter,
            Self::RunSelected => matches!(key.code, KeyCode::Enter | KeyCode::Char('e')),
            Self::RunBatch => key.code == KeyCode::Char('r'),
            Self::ToggleAll => key.code == KeyCode::Char('t'),
            Self::Clear => key.code == KeyCode::Char('c'),
            Self::Stop => matches!(key.code, KeyCode::Enter | KeyCode::Char('s')),
            Self::QuitRunning => matches!(key.code, KeyCode::Esc | KeyCode::Char('q')),
            Self::LogPageUp => key.code == KeyCode::PageUp,
            Self::LogPageDown => key.code == KeyCode::PageDown,
            Self::LogHome => key.code == KeyCode::Home,
            Self::LogEnd => key.code == KeyCode::End,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortcutHint {
    pub group: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub priority: u8,
}

#[derive(Debug, Clone, Copy)]
struct ActionSpec {
    action: Action,
    hint: Option<ShortcutHint>,
}

const fn shown(
    action: Action,
    group: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    priority: u8,
) -> ActionSpec {
    ActionSpec {
        action,
        hint: Some(ShortcutHint {
            group,
            key,
            label,
            description,
            priority,
        }),
    }
}

const fn hidden(action: Action) -> ActionSpec {
    ActionSpec { action, hint: None }
}

const RUNNING: &[ActionSpec] = &[
    shown(Action::Stop, "运行", "Enter/s", "停止", "停止当前任务", 0),
    shown(
        Action::LogPageUp,
        "导航",
        "PgUp/PgDn",
        "日志",
        "快速滚动日志",
        1,
    ),
    hidden(Action::LogPageDown),
    shown(
        Action::QuitRunning,
        "运行",
        "q/Esc",
        "退出",
        "停止任务并退出",
        3,
    ),
    shown(
        Action::LogHome,
        "导航",
        "Home/End",
        "首尾",
        "跳到日志开头或末尾",
        4,
    ),
    hidden(Action::LogEnd),
];
const MAIN: &[ActionSpec] = &[
    shown(
        Action::Previous,
        "导航",
        "↑↓/jk",
        "选择",
        "选择主菜单项目",
        0,
    ),
    hidden(Action::Next),
    shown(Action::Activate, "运行", "Enter", "确认", "打开当前项目", 1),
    shown(Action::Back, "导航", "q/Esc", "退出", "退出 MaaTUI", 3),
];
const DAILY: &[ActionSpec] = &[
    shown(
        Action::Previous,
        "导航",
        "↑↓/jk",
        "选择",
        "选择运行或配置",
        0,
    ),
    hidden(Action::Next),
    shown(Action::Activate, "运行", "Enter", "确认", "执行当前选择", 1),
    shown(Action::RunDaily, "运行", "r", "运行", "直接运行每日任务", 2),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回主菜单", 3),
    shown(Action::OpenConfig, "编辑", "c", "配置", "打开配置管理", 4),
];
const CONFIG: &[ActionSpec] = &[
    shown(
        Action::RunDailySingle,
        "运行",
        "x",
        "单跑",
        "单独执行当前选中的任务",
        0,
    ),
    shown(
        Action::MoveUp,
        "编辑",
        "Shift+↑↓",
        "移动",
        "调整任务顺序",
        6,
    ),
    hidden(Action::MoveDown),
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择任务", 0),
    hidden(Action::Next),
    shown(Action::Edit, "编辑", "Enter/e", "编辑", "编辑当前任务", 1),
    shown(
        Action::Toggle,
        "编辑",
        "Space",
        "开关",
        "启用或停用当前任务",
        2,
    ),
    shown(Action::Add, "编辑", "a", "新增", "新增任务", 3),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回每日任务", 4),
    shown(Action::Delete, "编辑", "d", "删除", "删除当前任务", 5),
    shown(Action::Reload, "编辑", "r", "重载", "重新加载配置", 7),
];
const ADD_TASK: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择任务类型", 0),
    hidden(Action::Next),
    shown(
        Action::Activate,
        "编辑",
        "Enter",
        "新增",
        "新增并编辑任务",
        1,
    ),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回配置管理", 3),
];
const TASK_EDIT: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择字段", 0),
    hidden(Action::Next),
    shown(Action::Edit, "编辑", "Enter/e", "编辑", "编辑当前字段", 1),
    shown(
        Action::PreviousEditorSection,
        "导航",
        "←→/hl",
        "切层",
        "切换设置层级",
        2,
    ),
    hidden(Action::NextEditorSection),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回配置管理", 3),
    shown(
        Action::CreateVariant,
        "编辑",
        "v",
        "建变体",
        "为当前关卡创建活动变体",
        5,
    ),
];
const TASK_VARIANTS: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择字段", 0),
    hidden(Action::Next),
    shown(
        Action::Edit,
        "编辑",
        "Enter/e",
        "变体",
        "打开条件与变体列表",
        1,
    ),
    shown(
        Action::PreviousEditorSection,
        "导航",
        "←→/hl",
        "切层",
        "切换设置层级",
        2,
    ),
    hidden(Action::NextEditorSection),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回配置管理", 3),
];
const VARIANT_LIST: &[ActionSpec] = &[
    shown(
        Action::MoveUp,
        "编辑",
        "Shift+↑↓",
        "移动",
        "调整变体顺序",
        6,
    ),
    hidden(Action::MoveDown),
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择变体", 0),
    hidden(Action::Next),
    shown(Action::Edit, "编辑", "Enter/e", "编辑", "编辑当前变体", 1),
    shown(Action::Add, "编辑", "a", "新增", "新增变体", 2),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回任务编辑", 3),
    shown(Action::Delete, "编辑", "d", "删除", "删除当前变体", 5),
];
const VARIANT_EDIT: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择变体字段", 0),
    hidden(Action::Next),
    shown(Action::Edit, "编辑", "Enter/e", "编辑", "编辑当前字段", 1),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回变体列表", 3),
];
const COPILOT_SINGLES: &[ActionSpec] = &[
    shown(
        Action::SearchSingle,
        "编辑",
        "e/a",
        "搜索",
        "搜索或替换当前单作业",
        0,
    ),
    shown(
        Action::RunSingle,
        "运行",
        "Enter",
        "运行",
        "运行当前单作业",
        1,
    ),
    shown(
        Action::ToggleDifficulty,
        "编辑",
        "Space",
        "难度",
        "切换普通或突袭模式",
        2,
    ),
    shown(
        Action::NextCopilotTab,
        "导航",
        "Tab/←→",
        "切页",
        "切换自动战斗页签",
        3,
    ),
    hidden(Action::PreviousCopilotTab),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回主菜单", 5),
    shown(
        Action::ShowDetail,
        "导航",
        "i",
        "详情",
        "查看当前作业详情",
        5,
    ),
];
const COPILOT_SETS: &[ActionSpec] = &[
    shown(
        Action::MoveUp,
        "编辑",
        "Shift+↑↓",
        "移动",
        "调整当前条目顺序",
        10,
    ),
    hidden(Action::MoveDown),
    shown(
        Action::Previous,
        "导航",
        "↑↓/jk",
        "选择",
        "选择作业集条目",
        0,
    ),
    hidden(Action::Next),
    shown(
        Action::RunSelected,
        "运行",
        "Enter/e",
        "单跑",
        "单独运行当前条目",
        1,
    ),
    shown(Action::RunBatch, "运行", "r", "批量", "运行全部启用条目", 2),
    shown(
        Action::Toggle,
        "编辑",
        "Space",
        "启停",
        "启用或停用当前条目",
        3,
    ),
    shown(
        Action::NextCopilotTab,
        "导航",
        "Tab/←→",
        "切页",
        "切换自动战斗页签",
        3,
    ),
    hidden(Action::PreviousCopilotTab),
    shown(Action::Add, "编辑", "a", "添加", "添加作业集或单个作业", 4),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回主菜单", 5),
    shown(
        Action::ShowDetail,
        "导航",
        "i",
        "详情",
        "查看当前条目详情",
        6,
    ),
    shown(Action::Delete, "编辑", "d", "删除", "删除当前条目", 7),
    shown(
        Action::ToggleAll,
        "批量",
        "t",
        "全启停",
        "全部启用或全部停用",
        8,
    ),
    shown(Action::Clear, "批量", "c", "清空", "清空全部作业集条目", 9),
];
const COPILOT_SETTINGS: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择运行设置", 0),
    hidden(Action::Next),
    shown(
        Action::Edit,
        "编辑",
        "Enter/e",
        "编辑",
        "编辑当前运行设置",
        1,
    ),
    shown(Action::RunBatch, "运行", "r", "批量", "运行全部启用条目", 2),
    shown(
        Action::NextCopilotTab,
        "导航",
        "Tab/←→",
        "切页",
        "切换自动战斗页签",
        3,
    ),
    hidden(Action::PreviousCopilotTab),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回主菜单", 5),
];
const UPDATE: &[ActionSpec] = &[
    shown(Action::Previous, "导航", "↑↓/jk", "选择", "选择更新类型", 0),
    hidden(Action::Next),
    shown(
        Action::Activate,
        "运行",
        "Enter",
        "更新",
        "执行当前更新操作",
        1,
    ),
    shown(Action::Back, "导航", "Esc/q", "返回", "返回主菜单", 3),
];

fn specs(context: ShortcutContext) -> &'static [ActionSpec] {
    match context {
        ShortcutContext::Running => RUNNING,
        ShortcutContext::Main => MAIN,
        ShortcutContext::Daily => DAILY,
        ShortcutContext::Config => CONFIG,
        ShortcutContext::AddTask => ADD_TASK,
        ShortcutContext::TaskEdit => TASK_EDIT,
        ShortcutContext::TaskVariants => TASK_VARIANTS,
        ShortcutContext::VariantList => VARIANT_LIST,
        ShortcutContext::VariantEdit => VARIANT_EDIT,
        ShortcutContext::CopilotSingles => COPILOT_SINGLES,
        ShortcutContext::CopilotSets => COPILOT_SETS,
        ShortcutContext::CopilotSettings => COPILOT_SETTINGS,
        ShortcutContext::Update => UPDATE,
    }
}

pub fn action_for(context: ShortcutContext, key: KeyEvent) -> Option<Action> {
    specs(context)
        .iter()
        .find(|spec| spec.action.matches(key))
        .map(|spec| spec.action)
}

pub fn hints(context: ShortcutContext) -> Vec<ShortcutHint> {
    specs(context).iter().filter_map(|spec| spec.hint).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised_key(key: &str) -> KeyEvent {
        let (code, modifiers) = match key {
            "Enter" | "Enter/e" | "Enter/s" => (KeyCode::Enter, KeyModifiers::NONE),
            "Esc/q" | "q/Esc" => (KeyCode::Esc, KeyModifiers::NONE),
            "↑↓/jk" => (KeyCode::Up, KeyModifiers::NONE),
            "Shift+↑↓" => (KeyCode::Up, KeyModifiers::SHIFT),
            "←→/hl" => (KeyCode::Left, KeyModifiers::NONE),
            "Tab/←→" => (KeyCode::Tab, KeyModifiers::NONE),
            "PgUp/PgDn" => (KeyCode::PageUp, KeyModifiers::NONE),
            "Home/End" => (KeyCode::Home, KeyModifiers::NONE),
            "Space" => (KeyCode::Char(' '), KeyModifiers::NONE),
            "e/a" => (KeyCode::Char('e'), KeyModifiers::NONE),
            "a" | "c" | "d" | "i" | "r" | "t" | "v" | "x" => (
                KeyCode::Char(key.chars().next().unwrap()),
                KeyModifiers::NONE,
            ),
            unknown => panic!("缺少快捷键测试映射: {unknown}"),
        };
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn config_single_run_uses_plain_x() {
        assert_eq!(
            action_for(
                ShortcutContext::Config,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            ),
            Some(Action::RunDailySingle)
        );
        assert_ne!(
            action_for(
                ShortcutContext::Config,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
            ),
            Some(Action::RunDailySingle)
        );
    }

    #[test]
    fn advertised_actions_are_reachable_by_their_primary_key() {
        for context in [
            ShortcutContext::Running,
            ShortcutContext::Main,
            ShortcutContext::Daily,
            ShortcutContext::Config,
            ShortcutContext::AddTask,
            ShortcutContext::TaskEdit,
            ShortcutContext::TaskVariants,
            ShortcutContext::VariantList,
            ShortcutContext::VariantEdit,
            ShortcutContext::CopilotSingles,
            ShortcutContext::CopilotSets,
            ShortcutContext::CopilotSettings,
            ShortcutContext::Update,
        ] {
            let shown = specs(context).iter().filter_map(|spec| {
                spec.hint
                    .map(|hint| (spec.action, hint.key, advertised_key(hint.key)))
            });
            for (action, label, key) in shown {
                assert_eq!(
                    action_for(context, key),
                    Some(action),
                    "context={context:?}, advertised={label}"
                );
            }
        }
    }
}
