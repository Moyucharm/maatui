//! 自动肉鸽页面的字段编辑、持久化和运行控制。

use super::*;
use crate::roguelike::{
    RoguelikeChoice, RoguelikeField, RoguelikeSection, RoguelikeValue, award_choices,
    blackflow_strategy_choices, blackflow_target_choices, display_value, foldartal_choices,
    mode_choices, paradigm_choices, role_choices, squad_choices, theme_choices, tri_state_choices,
    write_run_file,
};

impl App {
    pub fn roguelike_visible_fields(&self) -> Vec<RoguelikeField> {
        self.roguelike.visible_fields(self.roguelike_section())
    }

    pub fn roguelike_field_display(&self, field: RoguelikeField) -> String {
        display_value(&self.roguelike, field)
    }

    pub fn roguelike_control_row_count(&self) -> usize {
        self.roguelike
            .visible_fields(RoguelikeSection::Control)
            .len()
            .saturating_add(1)
    }

    pub fn roguelike_advanced_row_count(&self) -> usize {
        self.roguelike
            .visible_fields(RoguelikeSection::Advanced)
            .len()
    }

    pub(super) fn clamp_roguelike_selection(&mut self) {
        self.roguelike_idx = self
            .roguelike_idx
            .min(self.roguelike_control_row_count().saturating_sub(1));
        self.roguelike_advanced_idx = self
            .roguelike_advanced_idx
            .min(self.roguelike_advanced_row_count().saturating_sub(1));
    }

    pub(crate) fn handle_roguelike_key(&mut self, key: KeyEvent) {
        let context = match self.roguelike_section() {
            RoguelikeSection::Control => ShortcutContext::RoguelikeControl,
            RoguelikeSection::Advanced => ShortcutContext::RoguelikeAdvanced,
        };
        match action_for(context, key) {
            Some(Action::NextRoguelikeTab) => {
                self.roguelike_section_idx =
                    (self.roguelike_section_idx + 1) % RoguelikeSection::ALL.len();
                self.clamp_roguelike_selection();
                return;
            }
            Some(Action::PreviousRoguelikeTab) => {
                self.roguelike_section_idx = self.roguelike_section_idx.saturating_sub(1);
                self.clamp_roguelike_selection();
                return;
            }
            _ => {}
        }

        match self.roguelike_section() {
            RoguelikeSection::Control => self.handle_roguelike_control_key(key),
            RoguelikeSection::Advanced => self.handle_roguelike_advanced_key(key),
        }
    }

    fn handle_roguelike_control_key(&mut self, key: KeyEvent) {
        let fields = self.roguelike.visible_fields(RoguelikeSection::Control);
        match action_for(ShortcutContext::RoguelikeControl, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => {
                self.roguelike_idx = self.roguelike_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                self.roguelike_idx = (self.roguelike_idx + 1).min(fields.len());
            }
            Some(Action::RunRoguelike) => self.start_roguelike(),
            Some(Action::Edit | Action::Activate) if self.roguelike_idx == 0 => {
                self.start_roguelike();
            }
            Some(Action::Edit | Action::Activate) => {
                if let Some(field) = fields.get(self.roguelike_idx - 1).copied() {
                    self.open_roguelike_field(field);
                }
            }
            _ => {}
        }
    }

    fn handle_roguelike_advanced_key(&mut self, key: KeyEvent) {
        let fields = self.roguelike.visible_fields(RoguelikeSection::Advanced);
        match action_for(ShortcutContext::RoguelikeAdvanced, key) {
            Some(Action::Back) => self.screen = Screen::Main,
            Some(Action::Previous) => {
                self.roguelike_advanced_idx = self.roguelike_advanced_idx.saturating_sub(1);
            }
            Some(Action::Next) => {
                if !fields.is_empty() {
                    self.roguelike_advanced_idx =
                        (self.roguelike_advanced_idx + 1).min(fields.len() - 1);
                }
            }
            Some(Action::RunRoguelike) => self.start_roguelike(),
            Some(Action::Edit | Action::Activate) => {
                if let Some(field) = fields.get(self.roguelike_advanced_idx).copied() {
                    self.open_roguelike_field(field);
                }
            }
            _ => {}
        }
    }

    fn open_roguelike_field(&mut self, field: RoguelikeField) {
        let current = app_value(self.roguelike.field_value(field));
        if is_numeric_field(field) || is_text_field(field) {
            self.open_input(
                format!("编辑 {}", field.label()),
                current.display(),
                InputTarget::RoguelikeField(field),
            );
            return;
        }

        let choices = choices_for(field, self.roguelike.theme);
        let options = choices
            .into_iter()
            .filter_map(|choice| choice_to_select_option(field, choice))
            .collect();
        let allow_custom = is_custom_field(field);
        if is_multi_field(field) {
            self.open_multi_select(
                format!("编辑 {}", field.label()),
                current,
                options,
                allow_custom,
                InputTarget::RoguelikeField(field),
            );
        } else {
            self.open_select(
                format!("编辑 {}", field.label()),
                current,
                options,
                allow_custom,
                InputTarget::RoguelikeField(field),
            );
        }
    }

    pub(super) fn commit_roguelike_input(
        &mut self,
        field: RoguelikeField,
        input: &str,
    ) -> anyhow::Result<()> {
        let current = app_value(self.roguelike.field_value(field));
        let value = match current {
            FieldValue::Integer(_) => {
                FieldValue::Integer(input.trim().parse().context("请输入有效整数")?)
            }
            FieldValue::StringArray(_) => FieldValue::StringArray(
                input
                    .split([',', ';'])
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect(),
            ),
            _ => FieldValue::String(input.trim().to_string()),
        };
        self.set_roguelike_field_result(field, value)
    }

    pub(super) fn set_roguelike_field_result(
        &mut self,
        field: RoguelikeField,
        value: FieldValue,
    ) -> anyhow::Result<()> {
        let mut options = self.roguelike.clone();
        options.set_field_value(field, roguelike_value(value))?;
        let config = self
            .roguelike_config
            .as_mut()
            .context("自动肉鸽配置未加载")?;
        config.set_options(options.clone())?;
        self.roguelike = options;
        self.roguelike_error = None;
        self.clamp_roguelike_selection();
        self.last_failed = false;
        Ok(())
    }

    pub(super) fn start_roguelike(&mut self) {
        if self.roguelike_config.is_none() {
            self.status_error(
                self.roguelike_error
                    .clone()
                    .unwrap_or_else(|| "自动肉鸽配置未加载".to_string()),
            );
            return;
        }
        let (path, task_name) = match write_run_file(&self.roguelike) {
            Ok(run_file) => run_file,
            Err(error) => {
                self.status_error(format!("生成自动肉鸽任务失败: {error}"));
                return;
            }
        };
        let command = TaskCommand::roguelike(&task_name, path);
        if !self.start_command(command) {
            self.remove_temp_task_file();
        }
    }
}

fn app_value(value: RoguelikeValue) -> FieldValue {
    match value {
        RoguelikeValue::Bool(value) => FieldValue::Bool(value),
        RoguelikeValue::Integer(value) => FieldValue::Integer(value),
        RoguelikeValue::String(value) => FieldValue::String(value),
        RoguelikeValue::StringArray(values) => FieldValue::StringArray(values),
    }
}

fn roguelike_value(value: FieldValue) -> RoguelikeValue {
    match value {
        FieldValue::Bool(value) => RoguelikeValue::Bool(value),
        FieldValue::Integer(value) => RoguelikeValue::Integer(value),
        FieldValue::Float(value) => RoguelikeValue::String(value.to_string()),
        FieldValue::String(value) => RoguelikeValue::String(value),
        FieldValue::StringArray(values) => RoguelikeValue::StringArray(values),
        FieldValue::IntegerArray(values) => {
            RoguelikeValue::StringArray(values.into_iter().map(|value| value.to_string()).collect())
        }
    }
}

fn is_numeric_field(field: RoguelikeField) -> bool {
    matches!(
        field,
        RoguelikeField::Difficulty | RoguelikeField::StartsCount | RoguelikeField::InvestmentsCount
    )
}

fn is_text_field(field: RoguelikeField) -> bool {
    matches!(
        field,
        RoguelikeField::CoreChar
            | RoguelikeField::FirstFloorFoldartal
            | RoguelikeField::StartWithSeed
    )
}

fn is_multi_field(field: RoguelikeField) -> bool {
    matches!(
        field,
        RoguelikeField::CollectibleModeStartList
            | RoguelikeField::StartFoldartalList
            | RoguelikeField::ExpectedCollapsalParadigms
    )
}

fn is_custom_field(field: RoguelikeField) -> bool {
    matches!(
        field,
        RoguelikeField::CollectibleModeSquad
            | RoguelikeField::Squad
            | RoguelikeField::Roles
            | RoguelikeField::CoreChar
            | RoguelikeField::FirstFloorFoldartal
            | RoguelikeField::StartWithSeed
            | RoguelikeField::CollectibleModeStartList
            | RoguelikeField::StartFoldartalList
            | RoguelikeField::ExpectedCollapsalParadigms
    )
}

fn choices_for(
    field: RoguelikeField,
    theme: crate::roguelike::RoguelikeTheme,
) -> Vec<RoguelikeChoice> {
    match field {
        RoguelikeField::Theme => theme_choices(),
        RoguelikeField::Mode => mode_choices(theme),
        RoguelikeField::CollectibleModeSquad | RoguelikeField::Squad => squad_choices(theme),
        RoguelikeField::Roles => role_choices(),
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
        | RoguelikeField::DeepExplorationAutoIterate => tri_state_choices(),
        RoguelikeField::CollectibleModeStartList => award_choices(theme),
        RoguelikeField::StartFoldartalList | RoguelikeField::FirstFloorFoldartal => {
            foldartal_choices()
        }
        RoguelikeField::ExpectedCollapsalParadigms => paradigm_choices(),
        RoguelikeField::BlackflowStrategy => blackflow_strategy_choices(),
        RoguelikeField::BlackflowCultivationTarget => blackflow_target_choices(),
        _ => Vec::new(),
    }
}

fn choice_to_select_option(field: RoguelikeField, choice: RoguelikeChoice) -> Option<SelectOption> {
    let value = if field == RoguelikeField::Mode {
        FieldValue::Integer(choice.value.parse().ok()?)
    } else {
        FieldValue::String(choice.value)
    };
    Some(SelectOption {
        value,
        label: choice.label,
        description: choice.description,
    })
}
