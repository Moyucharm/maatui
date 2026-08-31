//! MaaCore 自动肉鸽配置、参数转换和临时任务文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::storage::{atomic_write, maa_config_dir};

pub const CONFIG_VERSION: u32 = 1;
pub const TASK_TYPE: &str = "Roguelike";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoguelikeTheme {
    #[default]
    #[serde(rename = "Phantom")]
    Phantom,
    #[serde(rename = "Mizuki")]
    Mizuki,
    #[serde(rename = "Sami")]
    Sami,
    #[serde(rename = "Sarkaz")]
    Sarkaz,
    #[serde(rename = "JieGarden")]
    JieGarden,
    #[serde(rename = "BlackFlow")]
    BlackFlow,
}

impl RoguelikeTheme {
    pub const ALL: [Self; 6] = [
        Self::Phantom,
        Self::Mizuki,
        Self::Sami,
        Self::Sarkaz,
        Self::JieGarden,
        Self::BlackFlow,
    ];

    pub fn value(self) -> &'static str {
        match self {
            Self::Phantom => "Phantom",
            Self::Mizuki => "Mizuki",
            Self::Sami => "Sami",
            Self::Sarkaz => "Sarkaz",
            Self::JieGarden => "JieGarden",
            Self::BlackFlow => "BlackFlow",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Phantom => "傀影与猩红血钻",
            Self::Mizuki => "水月与深蓝之树",
            Self::Sami => "探索者的银凇止境",
            Self::Sarkaz => "萨卡兹的无终奇语",
            Self::JieGarden => "岁的界园志异",
            Self::BlackFlow => "黑流树海",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.value() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoguelikeSection {
    Control,
    Advanced,
}

impl RoguelikeSection {
    pub const ALL: [Self; 2] = [Self::Control, Self::Advanced];

    pub fn label(self) -> &'static str {
        match self {
            Self::Control => "控制面板",
            Self::Advanced => "高级设置",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoguelikeField {
    Theme,
    Difficulty,
    Mode,
    CollectibleModeSquad,
    Squad,
    Roles,
    CoreChar,
    StartsCount,
    InvestmentEnabled,
    InvestmentsCount,
    StopWhenInvestmentFull,
    InvestmentWithMoreScore,
    UseSupport,
    UseNonfriendSupport,
    StopAtFinalBoss,
    StopAtMaxLevel,
    StartWithEliteTwo,
    OnlyStartWithEliteTwo,
    CollectibleModeShopping,
    CollectibleModeStartList,
    RefreshTraderWithDice,
    FirstFloorFoldartal,
    StartFoldartalList,
    UseFoldartal,
    CheckCollapsalParadigms,
    DoubleCheckCollapsalParadigms,
    ExpectedCollapsalParadigms,
    MonthlySquadAutoIterate,
    MonthlySquadCheckComms,
    DeepExplorationAutoIterate,
    StartWithSeed,
    BlackflowStrategy,
    BlackflowCultivationTarget,
}

impl RoguelikeField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Theme => "肉鸽主题",
            Self::Difficulty => "难度",
            Self::Mode => "策略",
            Self::CollectibleModeSquad => "烧水分队",
            Self::Squad => "开局分队",
            Self::Roles => "开局职业组",
            Self::CoreChar => "开局干员",
            Self::StartsCount => "探索次数上限",
            Self::InvestmentEnabled => "投资源石锭",
            Self::InvestmentsCount => "投资次数上限",
            Self::StopWhenInvestmentFull => "仓库满时停止",
            Self::InvestmentWithMoreScore => "投资后继续购物",
            Self::UseSupport => "使用助战干员",
            Self::UseNonfriendSupport => "允许非好友助战",
            Self::StopAtFinalBoss => "最终 Boss 前停止",
            Self::StopAtMaxLevel => "等级刷满后停止",
            Self::StartWithEliteTwo => "凹开局干员精二",
            Self::OnlyStartWithEliteTwo => "仅凹干员精二",
            Self::CollectibleModeShopping => "烧水启用购物",
            Self::CollectibleModeStartList => "凹开局奖励",
            Self::RefreshTraderWithDice => "用骰子刷新商店",
            Self::FirstFloorFoldartal => "第一层期望密文板",
            Self::StartFoldartalList => "开局期望密文板",
            Self::UseFoldartal => "使用密文板",
            Self::CheckCollapsalParadigms => "检测坍缩范式",
            Self::DoubleCheckCollapsalParadigms => "坍缩范式防漏检测",
            Self::ExpectedCollapsalParadigms => "期望坍缩范式",
            Self::MonthlySquadAutoIterate => "月度小队自动切换",
            Self::MonthlySquadCheckComms => "月度小队检查通信",
            Self::DeepExplorationAutoIterate => "深入调查自动切换",
            Self::StartWithSeed => "固定种子",
            Self::BlackflowStrategy => "黑流树海策略",
            Self::BlackflowCultivationTarget => "襁褓动物目标",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RoguelikeValue {
    Bool(bool),
    Integer(i64),
    String(String),
    StringArray(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct RoguelikeChoice {
    pub value: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RoguelikeOptions {
    pub theme: RoguelikeTheme,
    pub difficulty: i64,
    pub mode: i64,
    pub collectible_mode_squad: String,
    pub squad: String,
    pub roles: String,
    pub core_char: String,
    pub starts_count: i64,
    pub investment_enabled: Option<bool>,
    pub investments_count: i64,
    pub stop_when_investment_full: Option<bool>,
    pub investment_with_more_score: Option<bool>,
    pub use_support: Option<bool>,
    pub use_nonfriend_support: Option<bool>,
    pub stop_at_final_boss: Option<bool>,
    pub stop_at_max_level: Option<bool>,
    pub start_with_elite_two: Option<bool>,
    pub only_start_with_elite_two: Option<bool>,
    pub collectible_mode_shopping: Option<bool>,
    pub collectible_mode_start_list: Vec<String>,
    pub refresh_trader_with_dice: Option<bool>,
    pub first_floor_foldartal: String,
    pub start_foldartal_list: Vec<String>,
    pub use_foldartal: Option<bool>,
    pub check_collapsal_paradigms: Option<bool>,
    pub double_check_collapsal_paradigms: Option<bool>,
    pub expected_collapsal_paradigms: Vec<String>,
    pub monthly_squad_auto_iterate: Option<bool>,
    pub monthly_squad_check_comms: Option<bool>,
    pub deep_exploration_auto_iterate: Option<bool>,
    pub start_with_seed: String,
    pub blackflow_strategy: String,
    pub blackflow_cultivation_target: String,
}

impl Default for RoguelikeOptions {
    fn default() -> Self {
        Self {
            theme: RoguelikeTheme::default(),
            difficulty: -1,
            mode: 0,
            collectible_mode_squad: String::new(),
            squad: "指挥分队".to_string(),
            roles: "取长补短".to_string(),
            core_char: String::new(),
            starts_count: i32::MAX as i64,
            investment_enabled: None,
            investments_count: i32::MAX as i64,
            stop_when_investment_full: None,
            investment_with_more_score: None,
            use_support: None,
            use_nonfriend_support: None,
            stop_at_final_boss: None,
            stop_at_max_level: None,
            start_with_elite_two: None,
            only_start_with_elite_two: None,
            collectible_mode_shopping: None,
            collectible_mode_start_list: Vec::new(),
            refresh_trader_with_dice: None,
            first_floor_foldartal: String::new(),
            start_foldartal_list: Vec::new(),
            use_foldartal: None,
            check_collapsal_paradigms: None,
            double_check_collapsal_paradigms: None,
            expected_collapsal_paradigms: Vec::new(),
            monthly_squad_auto_iterate: None,
            monthly_squad_check_comms: None,
            deep_exploration_auto_iterate: None,
            start_with_seed: String::new(),
            blackflow_strategy: String::new(),
            blackflow_cultivation_target: "swaddled_cat".to_string(),
        }
    }
}

impl RoguelikeOptions {
    pub fn normalize(&mut self) {
        self.collectible_mode_start_list = normalize_strings(&self.collectible_mode_start_list);
        self.start_foldartal_list = normalize_strings(&self.start_foldartal_list);
        self.expected_collapsal_paradigms = normalize_strings(&self.expected_collapsal_paradigms);
        if !valid_modes(self.theme).contains(&self.mode) {
            self.mode = default_mode(self.theme);
        }
        if self.only_start_with_elite_two == Some(true) {
            self.start_with_elite_two = Some(true);
        }
    }

    pub fn validate(&self) -> Result<()> {
        if !valid_modes(self.theme).contains(&self.mode) {
            bail!(
                "{} 不支持策略 mode={}，请选择当前主题可用的策略",
                self.theme.label(),
                self.mode
            );
        }
        if !(-1..=i32::MAX as i64).contains(&self.difficulty) {
            bail!("难度必须为 -1 到 {}", i32::MAX);
        }
        if !(0..=i32::MAX as i64).contains(&self.starts_count) {
            bail!("探索次数上限必须为 0 到 {}", i32::MAX);
        }
        if !(0..=i32::MAX as i64).contains(&self.investments_count) {
            bail!("投资次数上限必须为 0 到 {}", i32::MAX);
        }
        if self.use_nonfriend_support == Some(true) && self.use_support != Some(true) {
            bail!("允许非好友助战前必须开启使用助战干员");
        }
        if self.only_start_with_elite_two == Some(true) && self.start_with_elite_two != Some(true) {
            bail!("仅凹干员精二前必须开启凹开局干员精二");
        }
        if self.theme == RoguelikeTheme::BlackFlow
            && !self.blackflow_strategy.is_empty()
            && !blackflow_strategy_values().contains(&self.blackflow_strategy.as_str())
        {
            bail!("黑流树海策略无效: {}", self.blackflow_strategy);
        }
        if self.theme == RoguelikeTheme::BlackFlow
            && !self.blackflow_cultivation_target.is_empty()
            && !blackflow_target_values().contains(&self.blackflow_cultivation_target.as_str())
        {
            bail!("襁褓动物目标无效: {}", self.blackflow_cultivation_target);
        }
        Ok(())
    }

    pub fn field_value(&self, field: RoguelikeField) -> RoguelikeValue {
        match field {
            RoguelikeField::Theme => RoguelikeValue::String(self.theme.value().to_string()),
            RoguelikeField::Difficulty => RoguelikeValue::Integer(self.difficulty),
            RoguelikeField::Mode => RoguelikeValue::Integer(self.mode),
            RoguelikeField::CollectibleModeSquad => {
                RoguelikeValue::String(self.collectible_mode_squad.clone())
            }
            RoguelikeField::Squad => RoguelikeValue::String(self.squad.clone()),
            RoguelikeField::Roles => RoguelikeValue::String(self.roles.clone()),
            RoguelikeField::CoreChar => RoguelikeValue::String(self.core_char.clone()),
            RoguelikeField::StartsCount => RoguelikeValue::Integer(self.starts_count),
            RoguelikeField::InvestmentEnabled => tri_state_value(self.investment_enabled),
            RoguelikeField::InvestmentsCount => RoguelikeValue::Integer(self.investments_count),
            RoguelikeField::StopWhenInvestmentFull => {
                tri_state_value(self.stop_when_investment_full)
            }
            RoguelikeField::InvestmentWithMoreScore => {
                tri_state_value(self.investment_with_more_score)
            }
            RoguelikeField::UseSupport => tri_state_value(self.use_support),
            RoguelikeField::UseNonfriendSupport => tri_state_value(self.use_nonfriend_support),
            RoguelikeField::StopAtFinalBoss => tri_state_value(self.stop_at_final_boss),
            RoguelikeField::StopAtMaxLevel => tri_state_value(self.stop_at_max_level),
            RoguelikeField::StartWithEliteTwo => tri_state_value(self.start_with_elite_two),
            RoguelikeField::OnlyStartWithEliteTwo => {
                tri_state_value(self.only_start_with_elite_two)
            }
            RoguelikeField::CollectibleModeShopping => {
                tri_state_value(self.collectible_mode_shopping)
            }
            RoguelikeField::CollectibleModeStartList => {
                RoguelikeValue::StringArray(self.collectible_mode_start_list.clone())
            }
            RoguelikeField::RefreshTraderWithDice => tri_state_value(self.refresh_trader_with_dice),
            RoguelikeField::FirstFloorFoldartal => {
                RoguelikeValue::String(self.first_floor_foldartal.clone())
            }
            RoguelikeField::StartFoldartalList => {
                RoguelikeValue::StringArray(self.start_foldartal_list.clone())
            }
            RoguelikeField::UseFoldartal => tri_state_value(self.use_foldartal),
            RoguelikeField::CheckCollapsalParadigms => {
                tri_state_value(self.check_collapsal_paradigms)
            }
            RoguelikeField::DoubleCheckCollapsalParadigms => {
                tri_state_value(self.double_check_collapsal_paradigms)
            }
            RoguelikeField::ExpectedCollapsalParadigms => {
                RoguelikeValue::StringArray(self.expected_collapsal_paradigms.clone())
            }
            RoguelikeField::MonthlySquadAutoIterate => {
                tri_state_value(self.monthly_squad_auto_iterate)
            }
            RoguelikeField::MonthlySquadCheckComms => {
                tri_state_value(self.monthly_squad_check_comms)
            }
            RoguelikeField::DeepExplorationAutoIterate => {
                tri_state_value(self.deep_exploration_auto_iterate)
            }
            RoguelikeField::StartWithSeed => RoguelikeValue::String(self.start_with_seed.clone()),
            RoguelikeField::BlackflowStrategy => {
                RoguelikeValue::String(self.blackflow_strategy.clone())
            }
            RoguelikeField::BlackflowCultivationTarget => {
                RoguelikeValue::String(self.blackflow_cultivation_target.clone())
            }
        }
    }

    pub fn set_field_value(&mut self, field: RoguelikeField, value: RoguelikeValue) -> Result<()> {
        match (field, value) {
            (RoguelikeField::Theme, RoguelikeValue::String(value)) => {
                self.theme = RoguelikeTheme::parse(value.trim()).context("无效的肉鸽主题")?;
                if !valid_modes(self.theme).contains(&self.mode) {
                    self.mode = default_mode(self.theme);
                }
            }
            (RoguelikeField::Difficulty, RoguelikeValue::Integer(value)) => self.difficulty = value,
            (RoguelikeField::Mode, RoguelikeValue::Integer(value)) => self.mode = value,
            (RoguelikeField::CollectibleModeSquad, RoguelikeValue::String(value)) => {
                self.collectible_mode_squad = value.trim().to_string();
            }
            (RoguelikeField::Squad, RoguelikeValue::String(value)) => {
                self.squad = value.trim().to_string();
            }
            (RoguelikeField::Roles, RoguelikeValue::String(value)) => {
                self.roles = value.trim().to_string();
            }
            (RoguelikeField::CoreChar, RoguelikeValue::String(value)) => {
                self.core_char = value.trim().to_string();
            }
            (RoguelikeField::StartsCount, RoguelikeValue::Integer(value)) => {
                self.starts_count = value
            }
            (RoguelikeField::InvestmentEnabled, value) => {
                self.investment_enabled = parse_tri_state(value)?;
            }
            (RoguelikeField::InvestmentsCount, RoguelikeValue::Integer(value)) => {
                self.investments_count = value;
            }
            (RoguelikeField::StopWhenInvestmentFull, value) => {
                self.stop_when_investment_full = parse_tri_state(value)?;
            }
            (RoguelikeField::InvestmentWithMoreScore, value) => {
                self.investment_with_more_score = parse_tri_state(value)?;
            }
            (RoguelikeField::UseSupport, value) => self.use_support = parse_tri_state(value)?,
            (RoguelikeField::UseNonfriendSupport, value) => {
                self.use_nonfriend_support = parse_tri_state(value)?;
            }
            (RoguelikeField::StopAtFinalBoss, value) => {
                self.stop_at_final_boss = parse_tri_state(value)?;
            }
            (RoguelikeField::StopAtMaxLevel, value) => {
                self.stop_at_max_level = parse_tri_state(value)?;
            }
            (RoguelikeField::StartWithEliteTwo, value) => {
                self.start_with_elite_two = parse_tri_state(value)?;
            }
            (RoguelikeField::OnlyStartWithEliteTwo, value) => {
                self.only_start_with_elite_two = parse_tri_state(value)?;
            }
            (RoguelikeField::CollectibleModeShopping, value) => {
                self.collectible_mode_shopping = parse_tri_state(value)?;
            }
            (RoguelikeField::CollectibleModeStartList, RoguelikeValue::StringArray(value)) => {
                self.collectible_mode_start_list = normalize_strings(&value)
            }
            (RoguelikeField::RefreshTraderWithDice, value) => {
                self.refresh_trader_with_dice = parse_tri_state(value)?;
            }
            (RoguelikeField::FirstFloorFoldartal, RoguelikeValue::String(value)) => {
                self.first_floor_foldartal = value.trim().to_string();
            }
            (RoguelikeField::StartFoldartalList, RoguelikeValue::StringArray(value)) => {
                self.start_foldartal_list = normalize_strings(&value);
            }
            (RoguelikeField::UseFoldartal, value) => self.use_foldartal = parse_tri_state(value)?,
            (RoguelikeField::CheckCollapsalParadigms, value) => {
                self.check_collapsal_paradigms = parse_tri_state(value)?;
            }
            (RoguelikeField::DoubleCheckCollapsalParadigms, value) => {
                self.double_check_collapsal_paradigms = parse_tri_state(value)?;
            }
            (RoguelikeField::ExpectedCollapsalParadigms, RoguelikeValue::StringArray(value)) => {
                self.expected_collapsal_paradigms = normalize_strings(&value)
            }
            (RoguelikeField::MonthlySquadAutoIterate, value) => {
                self.monthly_squad_auto_iterate = parse_tri_state(value)?;
            }
            (RoguelikeField::MonthlySquadCheckComms, value) => {
                self.monthly_squad_check_comms = parse_tri_state(value)?;
            }
            (RoguelikeField::DeepExplorationAutoIterate, value) => {
                self.deep_exploration_auto_iterate = parse_tri_state(value)?;
            }
            (RoguelikeField::StartWithSeed, RoguelikeValue::String(value)) => {
                self.start_with_seed = value.trim().to_string();
            }
            (RoguelikeField::BlackflowStrategy, RoguelikeValue::String(value)) => {
                self.blackflow_strategy = value.trim().to_string();
            }
            (RoguelikeField::BlackflowCultivationTarget, RoguelikeValue::String(value)) => {
                self.blackflow_cultivation_target = value.trim().to_string()
            }
            (field, value) => bail!("字段 {} 的值类型不正确: {value:?}", field.label()),
        }
        self.normalize();
        self.validate()
    }

    pub fn visible_fields(&self, section: RoguelikeSection) -> Vec<RoguelikeField> {
        match section {
            RoguelikeSection::Control => {
                let mut fields = vec![
                    RoguelikeField::Theme,
                    RoguelikeField::Difficulty,
                    RoguelikeField::Mode,
                ];
                if self.mode == 4 {
                    fields.push(RoguelikeField::CollectibleModeSquad);
                }
                fields.extend([
                    RoguelikeField::Squad,
                    RoguelikeField::Roles,
                    RoguelikeField::CoreChar,
                ]);
                fields
            }
            RoguelikeSection::Advanced => {
                let mut fields = vec![
                    RoguelikeField::StartsCount,
                    RoguelikeField::InvestmentEnabled,
                ];
                if self.mode == 1 {
                    fields.extend([
                        RoguelikeField::InvestmentsCount,
                        RoguelikeField::StopWhenInvestmentFull,
                        RoguelikeField::InvestmentWithMoreScore,
                    ]);
                }
                fields.push(RoguelikeField::UseSupport);
                if self.use_support == Some(true) {
                    fields.push(RoguelikeField::UseNonfriendSupport);
                }
                if self.theme != RoguelikeTheme::Phantom && self.mode == 0 {
                    fields.push(RoguelikeField::StopAtFinalBoss);
                }
                if self.mode == 0 {
                    fields.push(RoguelikeField::StopAtMaxLevel);
                }
                if self.mode == 4 {
                    fields.extend([
                        RoguelikeField::StartWithEliteTwo,
                        RoguelikeField::OnlyStartWithEliteTwo,
                        RoguelikeField::CollectibleModeShopping,
                    ]);
                    if self.only_start_with_elite_two != Some(true) {
                        fields.push(RoguelikeField::CollectibleModeStartList);
                    }
                }
                if self.theme == RoguelikeTheme::Mizuki {
                    fields.push(RoguelikeField::RefreshTraderWithDice);
                }
                if self.theme == RoguelikeTheme::Sami {
                    fields.push(RoguelikeField::FirstFloorFoldartal);
                    if self.mode == 4 {
                        fields.push(RoguelikeField::StartFoldartalList);
                    }
                    fields.push(RoguelikeField::UseFoldartal);
                    fields.push(RoguelikeField::CheckCollapsalParadigms);
                    if self.check_collapsal_paradigms == Some(true) {
                        fields.push(RoguelikeField::DoubleCheckCollapsalParadigms);
                    }
                    if self.mode == 5 {
                        fields.push(RoguelikeField::ExpectedCollapsalParadigms);
                    }
                }
                if self.mode == 6 {
                    fields.push(RoguelikeField::MonthlySquadAutoIterate);
                    if self.monthly_squad_auto_iterate == Some(true) {
                        fields.push(RoguelikeField::MonthlySquadCheckComms);
                    }
                }
                if self.mode == 7 {
                    fields.push(RoguelikeField::DeepExplorationAutoIterate);
                }
                if self.is_seed_supported() {
                    fields.push(RoguelikeField::StartWithSeed);
                }
                if self.theme == RoguelikeTheme::BlackFlow {
                    fields.push(RoguelikeField::BlackflowStrategy);
                    if self.blackflow_strategy == "baby_animal" {
                        fields.push(RoguelikeField::BlackflowCultivationTarget);
                    }
                }
                fields
            }
        }
    }

    fn is_seed_supported(&self) -> bool {
        self.theme == RoguelikeTheme::Sarkaz
            && self.mode == 1
            && matches!(self.squad.as_str(), "点刺成锭分队" | "后勤分队")
    }

    pub fn params(&self) -> Result<Value> {
        self.validate()?;
        let mut params = Map::new();
        params.insert("enable".to_string(), Value::Bool(true));
        params.insert(
            "theme".to_string(),
            Value::String(self.theme.value().to_string()),
        );
        params.insert("mode".to_string(), json!(self.mode));
        params.insert("difficulty".to_string(), json!(self.difficulty));
        insert_string(&mut params, "squad", &self.squad);
        insert_string(&mut params, "roles", &self.roles);
        params.insert(
            "core_char".to_string(),
            Value::String(self.core_char.clone()),
        );
        params.insert("starts_count".to_string(), json!(self.starts_count));
        insert_optional_bool(&mut params, "investment_enabled", self.investment_enabled);
        if self.mode == 1 {
            params.insert(
                "investments_count".to_string(),
                json!(self.investments_count),
            );
        }
        if self.mode == 0 {
            insert_optional_bool(&mut params, "stop_at_max_level", self.stop_at_max_level);
        }
        if self.theme != RoguelikeTheme::Phantom && self.mode == 0 {
            insert_optional_bool(&mut params, "stop_at_final_boss", self.stop_at_final_boss);
        }
        insert_optional_bool(&mut params, "use_support", self.use_support);
        if self.use_support == Some(true) {
            insert_optional_bool(
                &mut params,
                "use_nonfriend_support",
                self.use_nonfriend_support,
            );
        }

        if self.mode == 1 {
            insert_optional_bool(
                &mut params,
                "stop_when_investment_full",
                self.stop_when_investment_full,
            );
            insert_optional_bool(
                &mut params,
                "investment_with_more_score",
                self.investment_with_more_score,
            );
        }
        if self.mode == 4 {
            insert_string(
                &mut params,
                "collectible_mode_squad",
                &self.collectible_mode_squad,
            );
            insert_optional_bool(
                &mut params,
                "start_with_elite_two",
                self.start_with_elite_two,
            );
            insert_optional_bool(
                &mut params,
                "only_start_with_elite_two",
                self.only_start_with_elite_two,
            );
            insert_optional_bool(
                &mut params,
                "collectible_mode_shopping",
                self.collectible_mode_shopping,
            );
            params.insert(
                "collectible_mode_start_list".to_string(),
                award_object(&self.collectible_mode_start_list, self.theme),
            );
        }
        if self.theme == RoguelikeTheme::Mizuki {
            insert_optional_bool(
                &mut params,
                "refresh_trader_with_dice",
                self.refresh_trader_with_dice,
            );
        }
        if self.theme == RoguelikeTheme::Sami {
            insert_string(
                &mut params,
                "first_floor_foldartal",
                &self.first_floor_foldartal,
            );
            if self.mode == 4 && !self.start_foldartal_list.is_empty() {
                params.insert(
                    "start_foldartal_list".to_string(),
                    json!(self.start_foldartal_list),
                );
            }
            insert_optional_bool(&mut params, "use_foldartal", self.use_foldartal);
            insert_optional_bool(
                &mut params,
                "check_collapsal_paradigms",
                self.check_collapsal_paradigms,
            );
            if self.check_collapsal_paradigms == Some(true) {
                insert_optional_bool(
                    &mut params,
                    "double_check_collapsal_paradigms",
                    self.double_check_collapsal_paradigms,
                );
            }
            if self.mode == 5 && !self.expected_collapsal_paradigms.is_empty() {
                params.insert(
                    "expected_collapsal_paradigms".to_string(),
                    json!(self.expected_collapsal_paradigms),
                );
            }
        }
        if self.mode == 6 {
            insert_optional_bool(
                &mut params,
                "monthly_squad_auto_iterate",
                self.monthly_squad_auto_iterate,
            );
            if self.monthly_squad_auto_iterate == Some(true) {
                insert_optional_bool(
                    &mut params,
                    "monthly_squad_check_comms",
                    self.monthly_squad_check_comms,
                );
            }
        }
        if self.mode == 7 {
            insert_optional_bool(
                &mut params,
                "deep_exploration_auto_iterate",
                self.deep_exploration_auto_iterate,
            );
        }
        if self.is_seed_supported() && !self.start_with_seed.is_empty() {
            params.insert(
                "start_with_seed".to_string(),
                Value::String(self.start_with_seed.clone()),
            );
        }
        if self.theme == RoguelikeTheme::BlackFlow {
            insert_string(&mut params, "blackflow_strategy", &self.blackflow_strategy);
            if self.blackflow_strategy == "baby_animal" {
                insert_string(
                    &mut params,
                    "blackflow_cultivation_target",
                    &self.blackflow_cultivation_target,
                );
            }
        }
        Ok(Value::Object(params))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRoguelikeConfig {
    version: u32,
    #[serde(default)]
    settings: RoguelikeOptions,
}

pub struct RoguelikeConfig {
    path: PathBuf,
    trusted_root: PathBuf,
    options: RoguelikeOptions,
}

impl RoguelikeConfig {
    pub fn load_default() -> Result<Self> {
        let config_root = maa_config_dir()?;
        let path = config_root.join("maatui/roguelike.json");
        Self::load_with_trusted_root(path, config_root.join("maatui"))
    }

    #[cfg(test)]
    pub fn load(path: PathBuf) -> Result<Self> {
        let trusted_root = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Self::load_with_trusted_root(path, trusted_root)
    }

    fn load_with_trusted_root(path: PathBuf, trusted_root: PathBuf) -> Result<Self> {
        let options = if path.is_file() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取自动肉鸽配置失败: {}", path.display()))?;
            let file: StoredRoguelikeConfig = serde_json::from_str(&raw)
                .with_context(|| format!("解析自动肉鸽配置失败: {}", path.display()))?;
            if file.version != CONFIG_VERSION {
                bail!("不支持的自动肉鸽配置版本: {}", file.version);
            }
            let mut options = file.settings;
            options.normalize();
            options
        } else {
            RoguelikeOptions::default()
        };
        Ok(Self {
            path,
            trusted_root,
            options,
        })
    }

    pub fn options(&self) -> &RoguelikeOptions {
        &self.options
    }

    pub fn set_options(&mut self, options: RoguelikeOptions) -> Result<()> {
        let original = self.options.clone();
        self.options = options;
        if let Err(error) = self.save() {
            self.options = original;
            return Err(error);
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建自动肉鸽配置目录失败: {}", parent.display()))?;
        }
        let file = StoredRoguelikeConfig {
            version: CONFIG_VERSION,
            settings: self.options.clone(),
        };
        let content = format!("{}\n", serde_json::to_string_pretty(&file)?);
        atomic_write(&self.path, &self.trusted_root, content.as_bytes())
    }
}

pub fn write_run_file(options: &RoguelikeOptions) -> Result<(PathBuf, String)> {
    let config_root = maa_config_dir()?;
    let dir = config_root.join("tasks");
    write_run_file_into_with_root(options, &dir, &config_root)
}

#[cfg(test)]
fn write_run_file_into(options: &RoguelikeOptions, dir: &Path) -> Result<(PathBuf, String)> {
    write_run_file_into_with_root(options, dir, dir)
}

fn write_run_file_into_with_root(
    options: &RoguelikeOptions,
    dir: &Path,
    trusted_root: &Path,
) -> Result<(PathBuf, String)> {
    static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

    let params = options.params()?;
    fs::create_dir_all(dir)
        .with_context(|| format!("创建自动肉鸽运行目录失败: {}", dir.display()))?;
    let sequence = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let basename = format!("maatui-roguelike-{}-{sequence}", std::process::id());
    let path = dir.join(format!("{basename}.json"));
    let task = json!({
        "tasks": [{
            "name": "MaaTUI 自动肉鸽",
            "type": TASK_TYPE,
            "params": params,
        }]
    });
    let content = format!("{}\n", serde_json::to_string_pretty(&task)?);
    atomic_write(&path, trusted_root, content.as_bytes())?;
    Ok((path, basename))
}

pub fn theme_choices() -> Vec<RoguelikeChoice> {
    RoguelikeTheme::ALL
        .into_iter()
        .map(|theme| RoguelikeChoice {
            value: theme.value().to_string(),
            label: theme.label().to_string(),
            description: theme.value().to_string(),
        })
        .collect()
}

pub fn mode_choices(theme: RoguelikeTheme) -> Vec<RoguelikeChoice> {
    valid_modes(theme)
        .iter()
        .map(|&mode| RoguelikeChoice {
            value: mode.to_string(),
            label: mode_label(mode).to_string(),
            description: mode_description(mode).to_string(),
        })
        .collect()
}

pub fn squad_choices(theme: RoguelikeTheme) -> Vec<RoguelikeChoice> {
    let values: &[(&str, &str)] = match theme {
        RoguelikeTheme::Phantom => &[
            ("指挥分队", "指挥分队"),
            ("突击战术分队", "突击战术分队"),
            ("后勤分队", "后勤分队"),
            ("心胜于物分队", "心胜于物分队"),
        ],
        RoguelikeTheme::Mizuki => &[
            ("指挥分队", "指挥分队"),
            ("以人为本分队", "以人为本分队"),
            ("心胜于物分队", "心胜于物分队"),
            ("远程战术分队", "远程战术分队"),
            ("特训分队", "特训分队"),
        ],
        RoguelikeTheme::Sami => &[
            ("指挥分队", "指挥分队"),
            ("特训分队", "特训分队"),
            ("远程战术分队", "远程战术分队"),
            ("生活至上分队", "生活至上分队"),
            ("心胜于物分队", "心胜于物分队"),
        ],
        RoguelikeTheme::Sarkaz => &[
            ("指挥分队", "指挥分队"),
            ("蓝图测绘分队", "蓝图测绘分队"),
            ("远程战术分队", "远程战术分队"),
            ("点刺成锭分队", "点刺成锭分队"),
            ("后勤分队", "后勤分队"),
        ],
        RoguelikeTheme::JieGarden => &[
            ("指挥分队", "指挥分队"),
            ("远程战术分队", "远程战术分队"),
            ("突击战术分队", "突击战术分队"),
            ("后勤分队", "后勤分队"),
        ],
        RoguelikeTheme::BlackFlow => &[
            ("指挥分队", "指挥分队"),
            ("特勤分队", "特勤分队"),
            ("堡垒战术分队", "堡垒战术分队"),
            ("后勤分队", "后勤分队"),
        ],
    };
    values
        .iter()
        .map(|(value, label)| RoguelikeChoice {
            value: (*value).to_string(),
            label: (*label).to_string(),
            description: "可手动输入资源中存在的分队名".to_string(),
        })
        .collect()
}

pub fn role_choices() -> Vec<RoguelikeChoice> {
    [
        ("取长补短", "取长补短"),
        ("稳扎稳打", "稳扎稳打"),
        ("临机应变", "临机应变"),
        ("坚不可摧", "坚不可摧"),
        ("先手必胜", "先手必胜"),
    ]
    .into_iter()
    .map(|(value, label)| RoguelikeChoice {
        value: value.to_string(),
        label: label.to_string(),
        description: "可手动输入资源中存在的职业组名".to_string(),
    })
    .collect()
}

pub fn tri_state_choices() -> Vec<RoguelikeChoice> {
    [
        (
            "default",
            "跟随 Core 默认",
            "不主动覆盖 MaaCore 的上下文默认值",
        ),
        ("on", "开启", "明确向 MaaCore 传递 true"),
        ("off", "关闭", "明确向 MaaCore 传递 false"),
    ]
    .into_iter()
    .map(|(value, label, description)| RoguelikeChoice {
        value: value.to_string(),
        label: label.to_string(),
        description: description.to_string(),
    })
    .collect()
}

pub fn award_choices(theme: RoguelikeTheme) -> Vec<RoguelikeChoice> {
    let mut values = vec![
        ("hot_water", "热水壶", "通用奖励"),
        ("shield", "护盾", "通用奖励"),
        ("ingot", "源石锭", "通用奖励"),
        ("hope", "希望", "界园主题没有该奖励"),
        ("random", "随机收藏品", "消耗源石锭换取随机收藏品"),
    ];
    if theme == RoguelikeTheme::Mizuki {
        values.extend([
            ("key", "钥匙", "仅水月主题有效"),
            ("dice", "骰子", "仅水月主题有效"),
        ]);
    }
    if theme == RoguelikeTheme::Sarkaz {
        values.push(("ideas", "2 构想", "仅萨卡兹主题有效"));
    }
    if theme == RoguelikeTheme::JieGarden {
        values.push(("ticket", "票券", "仅界园主题有效"));
    }
    if theme == RoguelikeTheme::JieGarden {
        values.retain(|(value, _, _)| *value != "hope");
    }
    values
        .into_iter()
        .map(|(value, label, description)| RoguelikeChoice {
            value: value.to_string(),
            label: label.to_string(),
            description: description.to_string(),
        })
        .collect()
}

pub fn foldartal_choices() -> Vec<RoguelikeChoice> {
    [
        ("诞生的密文板", "诞生的密文板"),
        ("成长的密文板", "成长的密文板"),
        ("探索的密文板", "探索的密文板"),
        ("战斗的密文板", "战斗的密文板"),
        ("希望的密文板", "希望的密文板"),
    ]
    .into_iter()
    .map(|(value, label)| RoguelikeChoice {
        value: value.to_string(),
        label: label.to_string(),
        description: "可自定义输入密文板名称，名称需匹配资源".to_string(),
    })
    .collect()
}

pub fn paradigm_choices() -> Vec<RoguelikeChoice> {
    ["目空一些", "睁眼瞎", "图像损坏", "一抹黑"]
        .into_iter()
        .map(|value| RoguelikeChoice {
            value: value.to_string(),
            label: value.to_string(),
            description: "萨米主题坍缩范式名称".to_string(),
        })
        .collect()
}

pub fn blackflow_strategy_choices() -> Vec<RoguelikeChoice> {
    [
        ("", "跟随策略", "留空时由 MaaCore 按 mode 与投资设置推断"),
        (
            "baby_animal",
            "培育襁褓动物",
            "探索前几层并进入秘境行商培育种子",
        ),
        ("investment", "固定商店投资", "以最短路线抵达固定普通商店"),
        (
            "burn_with_investment",
            "投资后速刷",
            "第一层完成投资后尽快到达第三层",
        ),
        ("burn", "速刷第三层", "尽快到达第三层后重开"),
    ]
    .into_iter()
    .map(|(value, label, description)| RoguelikeChoice {
        value: value.to_string(),
        label: label.to_string(),
        description: description.to_string(),
    })
    .collect()
}

pub fn blackflow_target_choices() -> Vec<RoguelikeChoice> {
    [
        ("swaddled_cat", "襁褓猫"),
        ("swaddled_feathered_serpent", "襁褓羽蛇"),
        ("swaddled_dog", "襁褓犬"),
        ("swaddled_cerberus", "襁褓刻耳柏洛斯"),
    ]
    .into_iter()
    .map(|(value, label)| RoguelikeChoice {
        value: value.to_string(),
        label: label.to_string(),
        description: "黑流树海培育目标".to_string(),
    })
    .collect()
}

pub fn display_value(options: &RoguelikeOptions, field: RoguelikeField) -> String {
    match (field, options.field_value(field)) {
        (RoguelikeField::Theme, RoguelikeValue::String(value)) => {
            RoguelikeTheme::parse(&value).map_or(value, |theme| theme.label().to_string())
        }
        (RoguelikeField::Mode, RoguelikeValue::Integer(value)) => mode_label(value).to_string(),
        (_, RoguelikeValue::String(value)) if is_tri_state_field(field) => match value.as_str() {
            "on" => "开启".to_string(),
            "off" => "关闭".to_string(),
            _ => "跟随 Core 默认".to_string(),
        },
        (_, RoguelikeValue::StringArray(values))
            if field == RoguelikeField::CollectibleModeStartList =>
        {
            if values.is_empty() {
                "不指定".to_string()
            } else {
                values
                    .iter()
                    .map(|value| award_label(value))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
        (_, RoguelikeValue::StringArray(values)) => {
            if values.is_empty() {
                "不指定".to_string()
            } else {
                values.join(", ")
            }
        }
        (_, RoguelikeValue::String(value)) if value.is_empty() => "<空>".to_string(),
        (_, RoguelikeValue::String(value)) => value,
        (_, RoguelikeValue::Integer(value)) => value.to_string(),
        (_, RoguelikeValue::Bool(value)) => if value { "开启" } else { "关闭" }.to_string(),
    }
}

fn is_tri_state_field(field: RoguelikeField) -> bool {
    matches!(
        field,
        RoguelikeField::InvestmentEnabled
            | RoguelikeField::StopWhenInvestmentFull
            | RoguelikeField::InvestmentWithMoreScore
            | RoguelikeField::UseSupport
            | RoguelikeField::UseNonfriendSupport
            | RoguelikeField::StopAtFinalBoss
            | RoguelikeField::StopAtMaxLevel
            | RoguelikeField::StartWithEliteTwo
            | RoguelikeField::OnlyStartWithEliteTwo
            | RoguelikeField::CollectibleModeShopping
            | RoguelikeField::RefreshTraderWithDice
            | RoguelikeField::UseFoldartal
            | RoguelikeField::CheckCollapsalParadigms
            | RoguelikeField::DoubleCheckCollapsalParadigms
            | RoguelikeField::MonthlySquadAutoIterate
            | RoguelikeField::MonthlySquadCheckComms
            | RoguelikeField::DeepExplorationAutoIterate
    )
}

fn tri_state_value(value: Option<bool>) -> RoguelikeValue {
    RoguelikeValue::String(
        match value {
            None => "default",
            Some(true) => "on",
            Some(false) => "off",
        }
        .to_string(),
    )
}

fn parse_tri_state(value: RoguelikeValue) -> Result<Option<bool>> {
    match value {
        RoguelikeValue::Bool(value) => Ok(Some(value)),
        RoguelikeValue::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "default" | "auto" | "跟随默认" => Ok(None),
            "on" | "true" | "1" | "开" | "开启" => Ok(Some(true)),
            "off" | "false" | "0" | "关" | "关闭" => Ok(Some(false)),
            _ => bail!("请输入跟随 Core 默认、开启或关闭"),
        },
        value => bail!("三态开关值类型不正确: {value:?}"),
    }
}

fn valid_modes(theme: RoguelikeTheme) -> Vec<i64> {
    match theme {
        RoguelikeTheme::Sami => vec![0, 1, 4, 5, 6, 7],
        RoguelikeTheme::BlackFlow => vec![30001],
        _ => vec![0, 1, 4, 6, 7],
    }
}

fn default_mode(theme: RoguelikeTheme) -> i64 {
    match theme {
        RoguelikeTheme::BlackFlow => 30001,
        _ => 0,
    }
}

fn mode_label(mode: i64) -> &'static str {
    match mode {
        0 => "刷分 / 奖励点数",
        1 => "刷源石锭",
        4 => "凹开局奖励",
        5 => "刷坍缩范式（萨米）",
        6 => "月度小队",
        7 => "深入调查",
        30001 => "培育襁褓动物（黑流）",
        _ => "未知策略",
    }
}

fn mode_description(mode: i64) -> &'static str {
    match mode {
        0 => "稳定探索更多层数，获取积分与奖励点数",
        1 => "第一层投资后退出，刷取源石锭",
        4 => "凹开局奖励，成功后停止本轮尝试",
        5 => "仅萨米主题：检查并刷取坍缩范式",
        6 => "刷取月度小队相关奖励",
        7 => "刷取深入调查相关奖励",
        30001 => "仅黑流树海：培育襁褓动物",
        _ => "MaaCore 未提供该策略",
    }
}

fn normalize_strings(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split([',', ';']))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .fold(Vec::new(), |mut result, value| {
            if !result.contains(&value) {
                result.push(value);
            }
            result
        })
}

fn insert_string(params: &mut Map<String, Value>, key: &str, value: &str) {
    params.insert(key.to_string(), Value::String(value.to_string()));
}

fn insert_optional_bool(params: &mut Map<String, Value>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        params.insert(key.to_string(), Value::Bool(value));
    }
}

fn award_object(values: &[String], theme: RoguelikeTheme) -> Value {
    let allowed = award_choices(theme)
        .into_iter()
        .map(|choice| choice.value)
        .collect::<Vec<_>>();
    let selected = values.iter().map(String::as_str).collect::<Vec<_>>();
    let mut object = Map::new();
    for value in allowed {
        object.insert(
            value.clone(),
            Value::Bool(selected.contains(&value.as_str())),
        );
    }
    Value::Object(object)
}

fn award_label(value: &str) -> String {
    match value {
        "hot_water" => "热水壶",
        "shield" => "护盾",
        "ingot" => "源石锭",
        "hope" => "希望",
        "random" => "随机收藏品",
        "key" => "钥匙",
        "dice" => "骰子",
        "ideas" => "2 构想",
        "ticket" => "票券",
        other => other,
    }
    .to_string()
}

fn blackflow_strategy_values() -> [&'static str; 5] {
    [
        "",
        "baby_animal",
        "investment",
        "burn_with_investment",
        "burn",
    ]
}

fn blackflow_target_values() -> [&'static str; 4] {
    [
        "swaddled_cat",
        "swaddled_feathered_serpent",
        "swaddled_dog",
        "swaddled_cerberus",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("maatui-roguelike-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn defaults_follow_core_and_expose_common_controls() {
        let options = RoguelikeOptions::default();
        assert_eq!(options.theme, RoguelikeTheme::Phantom);
        assert_eq!(options.difficulty, -1);
        assert_eq!(options.mode, 0);
        assert_eq!(options.investment_enabled, None);
        assert!(
            options
                .visible_fields(RoguelikeSection::Control)
                .contains(&RoguelikeField::CoreChar)
        );
        assert!(
            options
                .visible_fields(RoguelikeSection::Advanced)
                .contains(&RoguelikeField::UseSupport)
        );
    }

    #[test]
    fn changing_theme_normalizes_theme_specific_mode() {
        let mut options = RoguelikeOptions {
            mode: 5,
            ..RoguelikeOptions::default()
        };
        options
            .set_field_value(
                RoguelikeField::Theme,
                RoguelikeValue::String("BlackFlow".to_string()),
            )
            .unwrap();
        assert_eq!(options.mode, 30001);
        assert_eq!(
            options.visible_fields(RoguelikeSection::Control)[2],
            RoguelikeField::Mode
        );
    }

    #[test]
    fn params_filter_theme_specific_fields_and_encode_awards_as_object() {
        let options = RoguelikeOptions {
            mode: 4,
            collectible_mode_start_list: vec!["hot_water".to_string(), "shield".to_string()],
            refresh_trader_with_dice: Some(true),
            start_with_seed: "seed".to_string(),
            ..RoguelikeOptions::default()
        };
        let params = options.params().unwrap();
        assert_eq!(params["mode"], 4);
        assert!(
            params
                .get("collectible_mode_start_list")
                .unwrap()
                .is_object()
        );
        assert_eq!(params["collectible_mode_start_list"]["hot_water"], true);
        assert!(params.get("refresh_trader_with_dice").is_none());
        assert!(params.get("start_with_seed").is_none());
        assert!(params.get("investment_enabled").is_none());
        assert!(params.get("investments_count").is_none());

        let support_options = RoguelikeOptions {
            use_support: Some(true),
            use_nonfriend_support: Some(true),
            ..options
        };
        let support_params = support_options.params().unwrap();
        assert_eq!(support_params["use_support"], true);
        assert_eq!(support_params["use_nonfriend_support"], true);
    }

    #[test]
    fn sami_collapse_settings_are_conditionally_serialized() {
        let options = RoguelikeOptions {
            theme: RoguelikeTheme::Sami,
            mode: 5,
            check_collapsal_paradigms: Some(true),
            double_check_collapsal_paradigms: Some(false),
            expected_collapsal_paradigms: vec!["目空一些".to_string()],
            ..RoguelikeOptions::default()
        };
        let params = options.params().unwrap();
        assert_eq!(params["check_collapsal_paradigms"], true);
        assert_eq!(params["double_check_collapsal_paradigms"], false);
        assert_eq!(params["expected_collapsal_paradigms"][0], "目空一些");
    }

    #[test]
    fn config_round_trip_and_save_failure_rolls_back_memory() {
        let dir = temp_dir();
        let path = dir.join("roguelike.json");
        let mut config = RoguelikeConfig::load(path.clone()).unwrap();
        let mut options = config.options().clone();
        options.mode = 1;
        config.set_options(options.clone()).unwrap();
        assert_eq!(
            RoguelikeConfig::load(path.clone()).unwrap().options().mode,
            1
        );

        let blocker = dir.join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        let mut broken = RoguelikeConfig::load(blocker.join("roguelike.json")).unwrap();
        let original = broken.options().clone();
        let result = broken.set_options(RoguelikeOptions {
            mode: 1,
            ..original.clone()
        });
        assert!(result.is_err());
        assert_eq!(broken.options(), &original);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_config_is_reported_without_overwriting_source() {
        let dir = temp_dir();
        let path = dir.join("roguelike.json");
        let original = "{ not valid json }\n";
        fs::write(&path, original).unwrap();

        assert!(RoguelikeConfig::load(path.clone()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_file_contains_one_roguelike_task_and_unique_name() {
        let dir = temp_dir();
        let options = RoguelikeOptions::default();
        let (first, first_name) = write_run_file_into(&options, &dir).unwrap();
        let (second, second_name) = write_run_file_into(&options, &dir).unwrap();
        assert_ne!(first, second);
        assert_ne!(first_name, second_name);
        let root: Value = serde_json::from_str(&fs::read_to_string(&first).unwrap()).unwrap();
        assert_eq!(root["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(root["tasks"][0]["type"], TASK_TYPE);
        assert_eq!(root["tasks"][0]["params"]["theme"], "Phantom");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn string_arrays_accept_separators_and_remove_duplicates() {
        let mut options = RoguelikeOptions::default();
        options
            .set_field_value(
                RoguelikeField::ExpectedCollapsalParadigms,
                RoguelikeValue::StringArray(vec!["a,b".to_string(), "b; c".to_string()]),
            )
            .unwrap();
        assert_eq!(options.expected_collapsal_paradigms, ["a", "b", "c"]);
    }
}
