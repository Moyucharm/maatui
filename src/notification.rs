//! Linux 桌面通知：通过 `notify-send` 汇报目标任务的终态。

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_INTERVAL: Duration = Duration::from_millis(25);

use crate::app::LogScope;
use crate::runner::TaskKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskOutcome<'a> {
    Completed,
    Failed(&'a str),
    ManuallyStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Urgency {
    Normal,
    Critical,
}

impl Urgency {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Normal => "dialog-information",
            Self::Critical => "dialog-error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopNotification {
    title: String,
    body: String,
    urgency: Urgency,
}

#[derive(Debug)]
pub(crate) struct NotificationResult {
    pub scope: LogScope,
    pub result: Result<(), String>,
}

struct NotificationJob {
    scope: LogScope,
    notification: DesktopNotification,
}

pub(crate) struct NotificationWorker {
    jobs: Option<SyncSender<NotificationJob>>,
    results: Receiver<NotificationResult>,
    handle: Option<thread::JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}

impl NotificationWorker {
    pub(crate) fn new() -> Result<Self, String> {
        let (job_tx, job_rx) = mpsc::sync_channel::<NotificationJob>(4);
        let (result_tx, result_rx) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let handle = thread::Builder::new()
            .name("desktop-notification-worker".to_string())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let result = run_notification_command(
                        "notify-send",
                        &job.notification.arguments(),
                        SEND_TIMEOUT,
                    );
                    if result_tx
                        .send(NotificationResult {
                            scope: job.scope,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    if worker_stopping.load(Ordering::Relaxed) {
                        break;
                    }
                }
            })
            .map_err(|error| format!("无法启动桌面通知线程: {error}"))?;
        Ok(Self {
            jobs: Some(job_tx),
            results: result_rx,
            handle: Some(handle),
            stopping,
        })
    }

    pub(crate) fn enqueue(
        &self,
        scope: LogScope,
        notification: DesktopNotification,
    ) -> Result<(), String> {
        let sender = self
            .jobs
            .as_ref()
            .ok_or_else(|| "通知线程已关闭".to_string())?;
        sender
            .try_send(NotificationJob {
                scope,
                notification,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "待处理通知已达到上限".to_string(),
                mpsc::TrySendError::Disconnected(_) => "通知线程已退出".to_string(),
            })
    }

    pub(crate) fn poll(&self) -> Vec<NotificationResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            results.push(result);
        }
        results
    }

    pub(crate) fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.jobs.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for NotificationWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl DesktopNotification {
    fn arguments(&self) -> Vec<String> {
        vec![
            "--app-name".to_string(),
            "MaaTUI".to_string(),
            "--urgency".to_string(),
            self.urgency.as_str().to_string(),
            "--icon".to_string(),
            self.urgency.icon().to_string(),
            self.title.clone(),
            self.body.clone(),
        ]
    }
}

fn run_notification_command(
    program: &str,
    arguments: &[String],
    timeout: Duration,
) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("无法启动 {program}: {error}"))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} 退出状态异常: {status}")),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} 超过 {} 毫秒未响应，已终止",
                    timeout.as_millis()
                ));
            }
            Ok(None) => thread::sleep(WAIT_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 {program} 退出失败: {error}"));
            }
        }
    }
}

pub(crate) fn for_task(
    kind: TaskKind,
    label: &str,
    outcome: TaskOutcome<'_>,
) -> Option<DesktopNotification> {
    if kind == TaskKind::Update || outcome == TaskOutcome::ManuallyStopped {
        return None;
    }

    let (title, body, urgency) = match outcome {
        TaskOutcome::Completed => (
            format!("MaaTUI · {label}完成"),
            format!("{label}已正常完成。"),
            Urgency::Normal,
        ),
        TaskOutcome::Failed(detail) => (
            format!("MaaTUI · {label}异常中断"),
            detail.to_string(),
            Urgency::Critical,
        ),
        TaskOutcome::ManuallyStopped => unreachable!("手动停止已提前返回"),
    };
    Some(DesktopNotification {
        title,
        body,
        urgency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_completion_builds_normal_notification() {
        let notification = for_task(TaskKind::Daily, "每日任务", TaskOutcome::Completed).unwrap();

        assert_eq!(notification.urgency, Urgency::Normal);
        assert_eq!(notification.title, "MaaTUI · 每日任务完成");
        assert!(notification.body.contains("正常完成"));
        assert_eq!(
            notification.arguments(),
            [
                "--app-name",
                "MaaTUI",
                "--urgency",
                "normal",
                "--icon",
                "dialog-information",
                "MaaTUI · 每日任务完成",
                "每日任务已正常完成。",
            ]
        );
    }

    #[test]
    fn copilot_failure_builds_critical_notification_with_reason() {
        let notification =
            for_task(TaskKind::Copilot, "自动战斗", TaskOutcome::Failed("exit 1")).unwrap();

        assert_eq!(notification.urgency, Urgency::Critical);
        assert!(notification.title.contains("自动战斗异常中断"));
        assert_eq!(notification.body, "exit 1");
    }

    #[test]
    fn roguelike_completion_builds_normal_notification() {
        let notification =
            for_task(TaskKind::Roguelike, "自动肉鸽", TaskOutcome::Completed).unwrap();

        assert_eq!(notification.urgency, Urgency::Normal);
        assert_eq!(notification.title, "MaaTUI · 自动肉鸽完成");
        assert_eq!(notification.body, "自动肉鸽已正常完成。");
    }

    #[test]
    fn manual_stop_and_update_tasks_do_not_notify() {
        assert!(for_task(TaskKind::Daily, "每日任务", TaskOutcome::ManuallyStopped).is_none());
        assert!(for_task(TaskKind::Update, "资源热更新", TaskOutcome::Completed).is_none());
        assert!(
            for_task(
                TaskKind::Update,
                "资源热更新",
                TaskOutcome::Failed("exit 1")
            )
            .is_none()
        );
    }

    #[test]
    fn notification_command_runs_in_background_and_times_out() {
        let started = Instant::now();
        let error = run_notification_command(
            "sh",
            &["-c".to_string(), "while :; do :; done".to_string()],
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(500));
        assert!(error.contains("超过 500 毫秒未响应"));
    }

    #[test]
    fn notification_command_reports_nonzero_status() {
        let error = run_notification_command(
            "sh",
            &["-c".to_string(), "exit 7".to_string()],
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert!(error.contains("exit status: 7"));
    }

    #[test]
    fn worker_reports_results_and_rejects_full_queue() {
        let mut worker = NotificationWorker::new().unwrap();
        let notification = for_task(TaskKind::Daily, "每日任务", TaskOutcome::Completed).unwrap();
        for _ in 0..4 {
            worker
                .enqueue(LogScope::Daily, notification.clone())
                .unwrap();
        }
        assert!(
            worker
                .enqueue(
                    LogScope::Daily,
                    for_task(TaskKind::Daily, "每日任务", TaskOutcome::Completed).unwrap(),
                )
                .is_err()
        );
        worker.shutdown();
    }
}
