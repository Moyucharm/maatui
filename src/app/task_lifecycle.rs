//! Runner 任务生命周期、后台事件和退出处理。

use super::*;
use crate::config::TaskSummary;

pub(crate) fn log_scope_for_command(command: &TaskCommand) -> LogScope {
    match command.kind {
        TaskKind::Daily => LogScope::Daily,
        TaskKind::Copilot => LogScope::Copilot,
        TaskKind::Roguelike => LogScope::Roguelike,
        TaskKind::Update => LogScope::Update,
    }
}

pub(super) fn is_resource_update_command(command: &TaskCommand) -> bool {
    command.args.first().is_some_and(|arg| arg == "hot-update")
        || command.label.contains("资源热更新")
}

/// 收集「完全禁用」（没有任何启用实例）的任务链名。
/// maa-cli 会把禁用任务的空跑回调转发为 "<chain> Start"/"<chain> Completed" 日志行，
/// 需要过滤这类噪音；同类型任务只要有任一启用则不过滤，避免误伤真实运行日志。
pub(crate) fn fully_disabled_chains(tasks: &[TaskSummary]) -> Vec<String> {
    let mut chains = Vec::new();
    for task in tasks.iter().filter(|task| !task.enabled) {
        let already = chains.contains(&task.task_type)
            || tasks
                .iter()
                .any(|other| other.enabled && other.task_type == task.task_type);
        if !already {
            chains.push(task.task_type.clone());
        }
    }
    chains
}

impl App {
    pub(super) fn start_command(&mut self, command: TaskCommand) -> bool {
        if command.kind == TaskKind::Daily
            && command.daily_single_index.is_none()
            && self.flush_config_save().is_err()
        {
            return false;
        }
        self.active_log_scope = log_scope_for_command(&command);
        let buffer = self.log_buffer_mut(self.active_log_scope);
        buffer.lines.clear();
        buffer.scroll = u16::MAX;
        buffer.auto_scroll = true;
        self.last_failed = false;
        self.active_task_kind = None;
        self.task_abort_error = None;
        self.saw_outdated_resource_error = false;
        self.saw_ocr_to_t0_error = false;
        self.disabled_task_chains.clear();
        if command.tracks_daily_progress() {
            let (tasks, total) = match command.daily_single_index {
                Some(index) => {
                    let tasks = self
                        .config
                        .as_ref()
                        .and_then(|config| config.task_summary(index))
                        .into_iter()
                        .collect::<Vec<_>>();
                    // 用户主动单独执行，即使原配置关闭也按单个任务计。
                    (tasks, 1)
                }
                None => {
                    let tasks = self
                        .config
                        .as_ref()
                        .map(DailyConfig::task_summaries)
                        .unwrap_or_default();
                    let total = tasks.iter().filter(|task| task.enabled).count();
                    self.disabled_task_chains = fully_disabled_chains(&tasks);
                    (tasks, total)
                }
            };
            self.run_progress = Some(RunProgress {
                current: 0,
                total,
                label: "等待任务开始".to_string(),
            });
            self.daily_run = Some(DailyRunState { tasks, total });
        }
        self.temp_task_file = command.cleanup.clone();
        self.pending_resource_check = is_resource_update_command(&command);
        self.resource_version_before = if self.pending_resource_check {
            read_hot_update_version()
        } else {
            None
        };
        self.push_log(LogLevel::System, format!("启动: {}", command.display()));
        self.status_text = format!("正在启动{}…", command.label);

        match RunningTask::spawn(&command) {
            Ok(task) => {
                self.task = Some(task);
                self.active_task_kind = Some(command.kind);
                self.phase = TaskPhase::Running;
                self.started_at = Some(Instant::now());
                self.animation_started = Instant::now();
                self.active_label = command.label;
                self.status_text = format!("{}运行中", self.active_label);
                true
            }
            Err(error) => {
                self.pending_resource_check = false;
                self.resource_version_before = None;
                self.remove_temp_task_file();
                self.push_log(LogLevel::Error, error.clone());
                self.status_text = "启动失败".to_string();
                self.last_failed = true;
                self.run_progress = None;
                self.daily_run = None;
                self.disabled_task_chains.clear();
                self.send_task_notification(
                    command.kind,
                    &command.label,
                    TaskOutcome::Failed(&format!("启动失败: {error}")),
                );
                false
            }
        }
    }

    pub(super) fn request_stop(&mut self) {
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

    pub(crate) fn abort_running_task(&mut self, message: String) {
        let abort_error = self
            .task_abort_error
            .get_or_insert_with(|| message.clone())
            .clone();
        if let Some(batch) = self.copilot_batch.as_mut() {
            batch.set_abort_error(abort_error);
        }
        self.request_stop();
        self.status_error(message);
    }

    pub(crate) fn send_task_notification(
        &mut self,
        kind: TaskKind,
        label: &str,
        outcome: TaskOutcome<'_>,
    ) {
        let Some(notification) = notification::for_task(kind, label, outcome) else {
            return;
        };
        let Some(worker) = self.notification_worker.as_ref() else {
            self.push_log(LogLevel::Warn, "发送桌面通知失败: 通知线程不可用");
            return;
        };
        if let Err(error) = worker.enqueue(self.active_log_scope, notification) {
            self.push_log(LogLevel::Warn, format!("发送桌面通知失败: {error}"));
        }
    }

    pub(crate) fn poll_notification_results(&mut self) {
        let results = self
            .notification_worker
            .as_ref()
            .map(NotificationWorker::poll)
            .unwrap_or_default();
        for result in results {
            if let Err(error) = result.result {
                self.push_log_to(
                    result.scope,
                    LogLevel::Warn,
                    format!("发送桌面通知失败: {error}"),
                );
            }
        }
    }

    pub(super) fn request_quit(&mut self) {
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
        self.poll_config_save_results();
        self.poll_notification_results();
        self.poll_stage_refresh();
        self.poll_copilot_import();
        let events = self
            .task
            .as_mut()
            .map_or_else(Vec::new, RunningTask::poll_events);
        for event in events {
            match event {
                RunnerEvent::Line { level, text } => {
                    if !self.is_disabled_chain_log_noise(&text) {
                        self.push_log(level, text);
                    }
                }
                RunnerEvent::LogReaderFailed { stream, error } => {
                    let message = format!("读取 MaaCore {stream} 日志失败: {error}");
                    self.push_log(LogLevel::Error, message.clone());
                    if self.phase != TaskPhase::Idle {
                        self.abort_running_task(message);
                    }
                }
                RunnerEvent::DailyTaskStarted { task_id, taskchain } => {
                    self.on_daily_task_started(task_id, &taskchain)
                }
                RunnerEvent::CopilotStageSucceeded => self.on_copilot_stage_succeeded(),
                RunnerEvent::ProgressFailed(error) => {
                    let message = format!("读取 MaaCore 运行进度失败，已停止任务: {error}");
                    self.abort_running_task(message);
                }
                RunnerEvent::Exited { code, stopped } => self.on_exited(code, stopped),
            }
        }
    }

    pub(crate) fn poll_stage_refresh(&mut self) {
        while let Ok(event) = self.stage_refresh_rx.try_recv() {
            self.stage_refreshing = false;
            match event {
                StageRefreshEvent::Updated {
                    catalog,
                    tasks_updated,
                } => {
                    self.stage_catalog = catalog;
                    self.stage_options_cache.borrow_mut().take();
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

    /// maa-cli 将禁用任务链的空跑回调转发为 "<chain> Start"/"<chain> Completed" 日志行，
    /// 判断给定日志行是否属于这类噪音。按尾部匹配，不依赖时间戳前缀的具体格式。
    pub(super) fn is_disabled_chain_log_noise(&self, text: &str) -> bool {
        self.disabled_task_chains.iter().any(|chain| {
            text.ends_with(&format!("{chain} Start"))
                || text.ends_with(&format!("{chain} Completed"))
        })
    }

    pub(crate) fn on_daily_task_started(&mut self, task_id: usize, taskchain: &str) {
        let Some(state) = self.daily_run.as_ref() else {
            return;
        };
        let total = state.total;
        // 防御：当前进度不超过分母（taskid 语义在不同 maa 版本下可能保留关闭任务的序号）。
        let current = task_id.min(total);
        let index = task_id.saturating_sub(1);
        let enabled_index = current.saturating_sub(1);
        // 优先按实际启用任务顺序取名称，避免关闭任务穿插及同类型任务重复时错位；
        // 若回调使用原配置索引，则退回原始索引匹配，最后使用任务类型标签。
        let label = state
            .tasks
            .iter()
            .filter(|task| task.enabled)
            .nth(enabled_index)
            .filter(|task| task.task_type == taskchain)
            .or_else(|| {
                state
                    .tasks
                    .get(index)
                    .filter(|task| task.task_type == taskchain)
            })
            .map(|task| task.name.clone())
            .unwrap_or_else(|| task_type_label(taskchain).to_string());
        self.run_progress = Some(RunProgress {
            current,
            total,
            label,
        });
    }

    pub(crate) fn on_copilot_stage_succeeded(&mut self) {
        if self.copilot_batch.is_none() {
            if let Some(progress) = self.run_progress.as_mut()
                && progress.current < progress.total
            {
                progress.current += 1;
            }
            return;
        }
        let Some(completion) = self
            .copilot_batch
            .as_ref()
            .and_then(CopilotBatchState::current_completion)
        else {
            return;
        };
        let index = completion.index;
        let total = completion.total;
        let completed = completion.completed;
        let name = self
            .copilot_cache
            .as_ref()
            .and_then(|cache| cache.entry(index))
            .map(|entry| entry.display_name())
            .unwrap_or_else(|| format!("作业 {}", index + 1));
        let result = self
            .copilot_cache
            .as_mut()
            .context("作业列表未加载")
            .and_then(|cache| cache.set_enabled(index, false));
        match result {
            Ok(()) => {
                if let Some(batch) = self.copilot_batch.as_mut() {
                    batch.mark_success();
                }
                self.push_log(
                    LogLevel::Success,
                    format!("已完成并关闭 [{completed}/{total}] {name}"),
                );
                self.status_text = if completed < total {
                    let next = self
                        .copilot_batch
                        .as_ref()
                        .and_then(|batch| match batch.position()? {
                            BatchPosition::Running { index, .. } => Some(index),
                            BatchPosition::Finishing { .. } => None,
                        })
                        .and_then(|index| self.copilot_cache.as_ref()?.entry(index))
                        .map(|entry| format!("{} · {}", entry.stage_name, entry.display_name()))
                        .unwrap_or_else(|| "未知作业".to_string());
                    self.run_progress = Some(RunProgress {
                        current: completed + 1,
                        total,
                        label: next.clone(),
                    });
                    format!("作业集运行中 · {}/{total} · {next}", completed + 1)
                } else {
                    if let Some(progress) = self.run_progress.as_mut() {
                        progress.current = total;
                        progress.label = "全部作业已完成，正在收尾".to_string();
                    }
                    format!("作业集运行中 · {total}/{total} · 正在收尾")
                };
            }
            Err(error) => {
                let message = format!("保存成功作业状态失败，已停止批次: {error}");
                self.abort_running_task(message);
            }
        }
    }

    pub(crate) fn on_exited(&mut self, code: Option<i32>, stopped: bool) {
        self.remove_temp_task_file();
        let code_text = code.map_or_else(|| "?".to_string(), |code| code.to_string());
        let task_kind = self.active_task_kind.take();
        let task_label = self.active_label.clone();
        let task_abort_error = self.task_abort_error.take();
        let check_resource = self.pending_resource_check;
        let version_before = self.resource_version_before.take();
        self.pending_resource_check = false;
        let mut batch_state_error = None;
        let mut batch_abort_error = None;
        if let Some(batch) = self.copilot_batch.take() {
            let exit = batch.finish(code, stopped);
            batch_abort_error = exit.abort_error;
            if !exit.reconcile_indices.is_empty() {
                let result = self
                    .copilot_cache
                    .as_mut()
                    .context("作业列表未加载")
                    .and_then(|cache| cache.set_enabled_many(&exit.reconcile_indices, false));
                if let Err(error) = result {
                    batch_state_error = Some(error.to_string());
                } else {
                    self.push_log(
                        LogLevel::Warn,
                        format!(
                            "批次整体成功；已补记 {} 个未收到逐关日志的成功项",
                            exit.reconcile_indices.len()
                        ),
                    );
                }
            }
            remove_batch_task(&exit.task_path);
        }

        let manually_stopped = stopped
            && task_abort_error.is_none()
            && batch_abort_error.is_none()
            && batch_state_error.is_none();
        let completed = !stopped
            && code == Some(0)
            && task_abort_error.is_none()
            && batch_abort_error.is_none()
            && batch_state_error.is_none();
        let failure_detail = task_abort_error
            .clone()
            .or_else(|| batch_abort_error.clone())
            .or_else(|| {
                batch_state_error
                    .as_ref()
                    .map(|error| format!("保存作业列表失败: {error}"))
            })
            .unwrap_or_else(|| format!("{task_label}异常结束 (exit {code_text})"));

        if manually_stopped {
            self.push_log(
                LogLevel::Warn,
                format!("{}已停止 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已停止 · exit {code_text}");
            self.last_failed = false;
        } else if completed {
            self.push_log(
                LogLevel::Success,
                format!("{}完成 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("已完成 · exit {code_text}");
            self.last_failed = false;
            if check_resource {
                self.report_resource_update_result(version_before.as_ref());
            }
        } else {
            self.push_log(
                LogLevel::Error,
                format!("{}异常结束 (exit {code_text})", self.active_label),
            );
            self.status_text = format!("失败 · exit {code_text}");
            self.last_failed = true;
            if let Some(error) = batch_abort_error.as_deref() {
                self.push_log(LogLevel::Error, error);
            }
            if let Some(error) = batch_state_error.as_deref() {
                self.push_log(LogLevel::Error, format!("保存作业列表失败: {error}"));
            }
            if self.saw_ocr_to_t0_error {
                self.push_log(LogLevel::System, OCR_TO_T0_HINT);
            } else if self.saw_outdated_resource_error {
                self.push_log(LogLevel::System, OUTDATED_RESOURCE_HINT);
            }
        }

        if let Some(kind) = task_kind {
            let outcome = if manually_stopped {
                TaskOutcome::ManuallyStopped
            } else if completed {
                TaskOutcome::Completed
            } else {
                TaskOutcome::Failed(&failure_detail)
            };
            self.send_task_notification(kind, &task_label, outcome);
        }

        self.task = None;
        self.phase = TaskPhase::Idle;
        self.started_at = None;
        self.active_label.clear();
        self.run_progress = None;
        self.daily_run = None;
        // 注意：disabled_task_chains 刻意不在退出时清空——进程退出后 stdout
        // 管道中残留的日志行仍会经 Line 事件到达，需继续过滤，直到下次启动任务时重置。
        self.saw_outdated_resource_error = false;
        self.saw_ocr_to_t0_error = false;
        let buffer = self.log_buffer_mut(self.active_log_scope);
        buffer.auto_scroll = true;
        buffer.scroll = u16::MAX;
    }

    pub(crate) fn report_resource_update_result(&mut self, before: Option<&HotUpdateVersion>) {
        match read_hot_update_version() {
            Some(after) => {
                let summary = after.summary();
                if before.is_some_and(|before| before == &after) {
                    self.push_log(
                        LogLevel::Warn,
                        "热更新报告成功，但本地 version.json 未变化，可能未拉到新提交",
                    );
                    self.status_text = format!("{summary} · 未变化");
                } else {
                    self.push_log(LogLevel::System, &summary);
                    self.status_text = summary;
                }
            }
            None => {
                self.push_log(
                    LogLevel::Warn,
                    "热更新完成，但无法读取本地 version.json；请检查 `maa dir hot-update`",
                );
            }
        }
        self.ensure_tile_pos_aliases();
    }

    pub(super) fn ensure_tile_pos_aliases(&mut self) {
        let report = tile_alias::ensure_stage_code_aliases();
        if let Some(summary) = report.summary() {
            let level = if report.errors.is_empty() {
                LogLevel::System
            } else {
                LogLevel::Warn
            };
            self.push_log(level, summary);
        }
    }

    /// 删除单任务执行生成的临时任务文件；删除失败仅记录日志，不阻断。
    pub(crate) fn remove_temp_task_file(&mut self) {
        if let Some(path) = self.temp_task_file.take()
            && let Err(error) = fs::remove_file(&path)
        {
            self.push_log(
                LogLevel::Warn,
                format!("清理临时任务文件失败 {}: {error}", path.display()),
            );
        }
    }

    pub fn cleanup(&mut self) {
        if let Some(task) = self.task.take() {
            task.force_cleanup();
        }
        if let Some(batch) = self.copilot_batch.take() {
            remove_batch_task(batch.task_path());
        }
        self.remove_temp_task_file();
        if self.config_dirty {
            let _ = self.flush_config_save();
        }
        if let Some(mut worker) = self.notification_worker.take() {
            worker.shutdown();
        }
        if let Some(mut worker) = self.config_save_worker.take() {
            worker.shutdown();
        }
        self.active_task_kind = None;
        self.task_abort_error = None;
        self.phase = TaskPhase::Idle;
    }

    pub fn spinner(&self) -> char {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = (self.animation_started.elapsed().as_millis() / 80) as usize;
        FRAMES[frame % FRAMES.len()]
    }
}
