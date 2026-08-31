//! App 输入框、选择框和字段提交。

use super::*;

impl App {
    pub(super) fn handle_input_key(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_select_key(&mut self, key: KeyEvent) {
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
            InputTarget::RoguelikeField(field) => self.set_roguelike_field_result(field, value),
        }
    }

    pub(super) fn open_field_editor(
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

    pub(super) fn handle_confirm_key(&mut self, key: KeyEvent) {
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
                        ConfirmTarget::ClearAvatarCache => {
                            if self.phase != TaskPhase::Idle {
                                self.status_error("任务运行中，无法清理干员头像缓存".to_string());
                            } else {
                                match clear_maa_avatar_cache() {
                                    Ok(0) => {
                                        self.last_failed = false;
                                        self.status_text = "干员头像缓存为空，无需清理".to_string();
                                    }
                                    Ok(removed) => {
                                        self.last_failed = false;
                                        self.status_text =
                                            format!("已清理 {removed} 个干员头像缓存");
                                        self.push_log(
                                            LogLevel::System,
                                            format!("已清理 {removed} 个 MaaCore 干员头像缓存"),
                                        );
                                    }
                                    Err(error) => self.status_error(error.to_string()),
                                }
                            }
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

    pub(super) fn create_stage_variant(&mut self) {
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

    pub(super) fn edit_current_task_field(&mut self) {
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
            InputTarget::RoguelikeField(field) => self.commit_roguelike_input(field, &dialog.value),
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

    pub(super) fn open_input(&mut self, title: String, value: String, target: InputTarget) {
        self.input = Some(InputDialog {
            title,
            value,
            target,
        });
    }
}
