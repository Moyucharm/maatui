//! Copilot 导入、详情和批量运行控制。

use super::*;

impl App {
    pub(super) fn apply_copilot_cache<F>(&mut self, operation: F) -> bool
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

    pub(super) fn start_copilot_import(
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

    pub(super) fn poll_copilot_import(&mut self) {
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

    pub(super) fn finish_copilot_import(
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
}
