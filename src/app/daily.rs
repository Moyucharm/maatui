//! Daily 配置字段、动态关卡选项和保存调度。

use super::*;

impl App {
    pub(super) fn dynamic_stage_options(&self) -> Vec<SelectOption> {
        let client = self
            .config
            .as_ref()
            .and_then(|config| config.param_value(self.config_idx, "client_type"))
            .and_then(|value| match value {
                FieldValue::String(value) if !value.trim().is_empty() => Some(value),
                _ => None,
            })
            .unwrap_or_else(|| "Official".to_string());
        if let Some((_, options)) = self
            .stage_options_cache
            .borrow()
            .as_ref()
            .filter(|(cached_client, _)| cached_client == &client)
        {
            return options.clone();
        }
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
        *self.stage_options_cache.borrow_mut() = Some((client, options.clone()));
        options
    }

    pub(super) fn open_stage_select(
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

    pub(super) fn open_multi_select(
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

    pub(super) fn open_select(
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

    pub(super) fn open_select_with(
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

    pub(super) fn refresh_stage_catalog(&mut self) {
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

    pub(super) fn can_report_stage_refresh_status(&self) -> bool {
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

    pub(super) fn set_task_field(&mut self, task: usize, field: &FieldSpec, value: FieldValue) {
        if let Err(error) = self.set_task_field_result(task, field, value) {
            self.status_error(error.to_string());
        }
    }

    pub(super) fn set_task_field_result(
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
        if field.key == "client_type" {
            self.stage_options_cache.borrow_mut().take();
        }
        if self.schedule_config_save() {
            self.last_failed = false;
        }
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

    pub(super) fn set_variant_field_result(
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
        if self.schedule_config_save() {
            self.last_failed = false;
        }
        Ok(())
    }

    pub(super) fn apply_config<F>(&mut self, operation: F) -> bool
    where
        F: FnOnce(&mut DailyConfig) -> anyhow::Result<()>,
    {
        let result = self
            .config
            .as_mut()
            .context("daily 配置未加载")
            .and_then(|config| {
                if let Some(parent) = config.path().parent()
                    && !parent.is_dir()
                {
                    anyhow::bail!("配置目录不存在: {}", parent.display());
                }
                config.set_defer_save(true);
                operation(config)
            });
        match result {
            Ok(()) => {
                if self.schedule_config_save() {
                    self.last_failed = false;
                }
                true
            }
            Err(error) => {
                self.status_error(error.to_string());
                false
            }
        }
    }

    pub(super) fn schedule_config_save(&mut self) -> bool {
        if self.config.is_none() {
            return false;
        }
        self.config_revision = self.config_revision.saturating_add(1);
        self.config_dirty = true;
        self.config_failed_revision = None;
        if let Err(error) = self.enqueue_current_config_save(self.config_revision) {
            self.report_config_save_failure(error);
            false
        } else {
            true
        }
    }

    fn enqueue_current_config_save(&mut self, revision: u64) -> Result<(), String> {
        let (path, trusted_root, content) = {
            let config = self
                .config
                .as_ref()
                .ok_or_else(|| "daily 配置未加载".to_string())?;
            let content = config
                .serialize_snapshot()
                .map_err(|error| format!("序列化 daily 配置失败: {error}"))?;
            (
                config.path().to_path_buf(),
                config.trusted_root().to_path_buf(),
                content.into_bytes(),
            )
        };
        let Some(worker) = self.config_save_worker.as_ref() else {
            return Err("后台配置保存线程不可用，修改尚未写入磁盘".to_string());
        };
        worker.enqueue(revision, path, trusted_root, content);
        self.config_enqueued_revision = Some(revision);
        Ok(())
    }

    pub(super) fn report_config_save_failure(&mut self, error: String) {
        self.config_dirty = true;
        self.config_failed_revision = Some(self.config_revision);
        self.background_status_error_to(LogScope::Daily, error);
    }

    fn apply_config_save_result(&mut self, result: config_save::SaveResult) {
        if result.revision < self.config_revision {
            return;
        }
        match result.result {
            Ok(()) => {
                self.config_dirty = false;
                self.config_failed_revision = None;
                if self
                    .config_enqueued_revision
                    .is_some_and(|revision| revision <= result.revision)
                {
                    self.config_enqueued_revision = None;
                }
            }
            Err(error) => {
                self.config_dirty = true;
                self.config_failed_revision = Some(result.revision);
                self.background_status_error_to(
                    LogScope::Daily,
                    format!("后台保存 daily 配置失败: {error}"),
                );
            }
        }
    }

    /// 同步等待当前配置 revision 落盘；失败时保留 dirty 状态并允许重试。
    pub(super) fn flush_config_save(&mut self) -> Result<(), String> {
        if !self.config_dirty {
            return Ok(());
        }
        let revision = self.config_revision;
        if self.config_save_worker.is_none() {
            let error = "后台配置保存线程不可用，无法确认配置已写入磁盘".to_string();
            self.report_config_save_failure(error.clone());
            return Err(error);
        }
        if (self.config_enqueued_revision != Some(revision)
            || self.config_failed_revision == Some(revision))
            && let Err(error) = self.enqueue_current_config_save(revision)
        {
            self.report_config_save_failure(error.clone());
            return Err(error);
        }

        let result = self
            .config_save_worker
            .as_ref()
            .expect("已检查配置保存线程存在")
            .flush_until(revision);
        match result {
            Ok(saved_revision) => {
                self.apply_config_save_result(config_save::SaveResult {
                    revision: saved_revision,
                    result: Ok(()),
                });
                Ok(())
            }
            Err(error) => {
                self.apply_config_save_result(config_save::SaveResult {
                    revision,
                    result: Err(error.clone()),
                });
                Err(error)
            }
        }
    }

    pub(super) fn poll_config_save_results(&mut self) {
        let results = self
            .config_save_worker
            .as_ref()
            .map(config_save::ConfigSaveWorker::poll)
            .unwrap_or_default();
        for result in results {
            self.apply_config_save_result(result);
        }
    }
}
