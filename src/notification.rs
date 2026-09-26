//! Linux 桌面通知：通过 `notify-send` 汇报目标任务的终态。

use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const WAIT_INTERVAL: Duration = Duration::from_millis(25);

use crate::app::LogScope;
use crate::runner::TaskKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskOutcome<'a> {
    Completed,
    DailyItemFailed { name: &'a str, detail: &'a str },
    DailyPartiallyCompleted { failed_names: &'a str },
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
    jobs: Option<Sender<NotificationJob>>,
    results: Receiver<NotificationResult>,
    handle: Option<thread::JoinHandle<()>>,
}

impl NotificationWorker {
    pub(crate) fn new() -> Result<Self, String> {
        Self::spawn_worker("notify-send")
    }

    #[cfg(test)]
    pub(crate) fn new_with_program(program: &str) -> Result<Self, String> {
        Self::spawn_worker(program)
    }

    fn spawn_worker(program: &str) -> Result<Self, String> {
        let (job_tx, job_rx) = mpsc::channel::<NotificationJob>();
        let (result_tx, result_rx) = mpsc::channel();
        let worker_program = program.to_string();
        let handle = thread::Builder::new()
            .name("desktop-notification-worker".to_string())
            .spawn(move || {
                // 关闭时发送端被丢弃：recv 先排空已入队通知，再返回错误退出。
                while let Ok(job) = job_rx.recv() {
                    let result = run_notification_command(
                        &worker_program,
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
                }
            })
            .map_err(|error| format!("无法启动桌面通知线程: {error}"))?;
        Ok(Self {
            jobs: Some(job_tx),
            results: result_rx,
            handle: Some(handle),
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
            .send(NotificationJob {
                scope,
                notification,
            })
            .map_err(|_| "通知线程已退出".to_string())
    }

    pub(crate) fn poll(&self) -> Vec<NotificationResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            results.push(result);
        }
        results
    }

    pub(crate) fn shutdown(&mut self) {
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
        TaskOutcome::Completed if kind == TaskKind::Daily => (
            "MaaTUI · 每日任务-已完成".to_string(),
            "每日任务已正常完成。".to_string(),
            Urgency::Normal,
        ),
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
        TaskOutcome::DailyItemFailed { name, detail } => (
            format!("MaaTUI · 每日任务-{name}-执行失败"),
            detail.to_string(),
            Urgency::Critical,
        ),
        TaskOutcome::DailyPartiallyCompleted { failed_names } => (
            "MaaTUI · 每日任务-已完成 ⚠️部分过程失败".to_string(),
            format!("失败项：{failed_names}"),
            Urgency::Critical,
        ),
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn daily_completion_builds_normal_notification() {
        let notification = for_task(TaskKind::Daily, "每日任务", TaskOutcome::Completed).unwrap();

        assert_eq!(notification.urgency, Urgency::Normal);
        assert_eq!(notification.title, "MaaTUI · 每日任务-已完成");
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
                "MaaTUI · 每日任务-已完成",
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
    fn partial_daily_failure_builds_item_and_partial_notifications() {
        let item = for_task(
            TaskKind::Daily,
            "每日任务",
            TaskOutcome::DailyItemFailed {
                name: "领取奖励",
                detail: "关卡导航失败（1-7）",
            },
        )
        .unwrap();
        assert_eq!(item.title, "MaaTUI · 每日任务-领取奖励-执行失败");
        assert_eq!(item.body, "关卡导航失败（1-7）");
        assert_eq!(item.urgency, Urgency::Critical);

        let partial = for_task(
            TaskKind::Daily,
            "每日任务",
            TaskOutcome::DailyPartiallyCompleted {
                failed_names: "刷理智、领取奖励",
            },
        )
        .unwrap();
        assert_eq!(partial.title, "MaaTUI · 每日任务-已完成 ⚠️部分过程失败");
        assert_eq!(partial.body, "失败项：刷理智、领取奖励");
        assert_eq!(partial.urgency, Urgency::Critical);
    }

    #[test]
    fn worker_drains_all_queued_notifications_in_order() {
        let dir = temp_dir();
        let record = dir.join("record.txt");
        let mut worker = recording_worker(&dir, &record);
        for index in 0..6 {
            worker
                .enqueue(
                    LogScope::Daily,
                    for_task(
                        TaskKind::Daily,
                        "每日任务",
                        TaskOutcome::DailyItemFailed {
                            name: &format!("任务{index}"),
                            detail: "原因",
                        },
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        worker.shutdown();

        let recorded = fs::read_to_string(&record).unwrap();
        let lines: Vec<&str> = recorded.lines().collect();
        assert_eq!(lines.len(), 6, "recorded={recorded}");
        for (index, line) in lines.iter().enumerate() {
            assert_eq!(
                *line,
                format!("MaaTUI · 每日任务-任务{index}-执行失败|原因")
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    fn recording_worker(dir: &Path, record: &Path) -> NotificationWorker {
        let script = dir.join("recording-notify.sh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' \"$7\" \"$8\" >> {}\n",
                record.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        NotificationWorker::new_with_program(script.to_str().unwrap()).unwrap()
    }

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("maatui-notify-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
