//! 应用状态、按键处理与状态迁移。

use std::collections::VecDeque;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::runner::{LogStream, RunnerEvent, RunningTask};

/// 日志环形缓冲上限。
const MAX_LOG_LINES: usize = 3000;

/// 菜单项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Daily,
    Quit,
}

impl MenuItem {
    pub const ALL: [MenuItem; 2] = [MenuItem::Daily, MenuItem::Quit];

    pub fn label(self) -> &'static str {
        match self {
            MenuItem::Daily => "每日任务",
            MenuItem::Quit => "退出",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            MenuItem::Daily => "maa run daily -v",
            MenuItem::Quit => "安全退出 MaaTUI",
        }
    }
}

/// 任务运行相位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    Idle,
    Running,
    Stopping,
}

/// 单条日志。
#[derive(Debug, Clone)]
pub struct LogLine {
    pub stream: LogStream,
    pub text: String,
}

/// 应用整体状态。
pub struct App {
    pub menu_idx: usize,
    pub phase: TaskPhase,
    pub logs: VecDeque<LogLine>,
    /// 从顶部算起的滚动量（0 = 最顶；最大使底部可见）。
    pub scroll: u16,
    pub auto_scroll: bool,
    pub status_text: String,
    pub should_quit: bool,
    pub frame_tick: u64,
    pub started_at: Option<Instant>,
    task: Option<RunningTask>,
    /// 上一次结束是否失败（影响状态色点）。
    pub last_failed: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            menu_idx: 0,
            phase: TaskPhase::Idle,
            logs: VecDeque::new(),
            scroll: 0,
            auto_scroll: true,
            status_text: "就绪".to_string(),
            should_quit: false,
            frame_tick: 0,
            started_at: None,
            task: None,
            last_failed: false,
        }
    }

    pub fn selected(&self) -> MenuItem {
        MenuItem::ALL[self.menu_idx.min(MenuItem::ALL.len() - 1)]
    }

    pub fn push_log(&mut self, stream: LogStream, text: impl Into<String>) {
        self.logs.push_back(LogLine {
            stream,
            text: text.into(),
        });
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
        if self.auto_scroll {
            self.scroll = self.max_scroll(0);
        }
    }

    /// 根据可视行数计算最大 scroll（逻辑行）。
    pub fn max_scroll(&self, visible_rows: u16) -> u16 {
        let total = self.logs.len() as u16;
        total.saturating_sub(visible_rows.max(1))
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Char('s') => self.request_stop(),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.phase == TaskPhase::Idle && self.menu_idx > 0 {
                    self.menu_idx -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.phase == TaskPhase::Idle && self.menu_idx + 1 < MenuItem::ALL.len() {
                    self.menu_idx += 1;
                }
            }
            KeyCode::Enter => self.activate_menu(),
            KeyCode::PageUp => self.scroll_logs_up(10),
            KeyCode::PageDown => self.scroll_logs_down(10),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_logs_up(8);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_logs_down(8);
            }
            KeyCode::Home => {
                self.auto_scroll = false;
                self.scroll = 0;
            }
            KeyCode::End => {
                self.auto_scroll = true;
                self.scroll = u16::MAX; // 渲染时钳制
            }
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_logs_up(3),
            MouseEventKind::ScrollDown => self.scroll_logs_down(3),
            _ => {}
        }
    }

    fn scroll_logs_up(&mut self, n: u16) {
        self.auto_scroll = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    fn scroll_logs_down(&mut self, n: u16) {
        self.scroll = self.scroll.saturating_add(n);
        // 是否恢复 auto_scroll 由 ui 在得知 visible_rows 后钳制时判断。
    }

    fn activate_menu(&mut self) {
        if self.phase != TaskPhase::Idle {
            return;
        }
        match self.selected() {
            MenuItem::Daily => self.start_daily(),
            MenuItem::Quit => self.request_quit(),
        }
    }

    fn start_daily(&mut self) {
        self.logs.clear();
        self.scroll = 0;
        self.auto_scroll = true;
        self.last_failed = false;
        self.push_log(LogStream::System, ">>> 启动: maa run daily -v");
        self.status_text = "正在启动每日任务…".to_string();

        match RunningTask::spawn_daily() {
            Ok(task) => {
                self.task = Some(task);
                self.phase = TaskPhase::Running;
                self.started_at = Some(Instant::now());
                self.status_text = "每日任务运行中".to_string();
                self.menu_idx = 0;
            }
            Err(err) => {
                self.push_log(LogStream::System, format!("错误: {err}"));
                self.status_text = "启动失败".to_string();
                self.last_failed = true;
                self.phase = TaskPhase::Idle;
            }
        }
    }

    fn request_stop(&mut self) {
        if let Some(task) = self.task.as_mut()
            && !task.is_finished()
            && !task.stop_requested()
        {
            task.request_stop();
            self.phase = TaskPhase::Stopping;
            self.status_text = "正在停止…".to_string();
            self.push_log(LogStream::System, ">>> 已发送停止信号 (SIGTERM)");
        }
    }

    fn request_quit(&mut self) {
        match self.phase {
            TaskPhase::Idle => {
                self.should_quit = true;
            }
            TaskPhase::Running | TaskPhase::Stopping => {
                self.request_stop();
                self.should_quit = true;
                self.push_log(LogStream::System, ">>> 退出前停止任务…");
            }
        }
    }

    /// 每 tick 推进 runner 事件。
    pub fn tick(&mut self) {
        self.frame_tick = self.frame_tick.wrapping_add(1);

        let events = {
            let Some(task) = self.task.as_mut() else {
                return;
            };
            task.poll_events()
        };

        for ev in events {
            match ev {
                RunnerEvent::Line { stream, text } => {
                    self.push_log(stream, text);
                }
                RunnerEvent::Exited { code, stopped } => {
                    self.on_exited(code, stopped);
                }
            }
        }
    }

    fn on_exited(&mut self, code: Option<i32>, stopped: bool) {
        let code_str = code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".to_string());

        if stopped {
            self.push_log(
                LogStream::System,
                format!(">>> 任务已停止 (exit {code_str})"),
            );
            self.status_text = format!("已停止 · exit {code_str}");
            self.last_failed = false;
        } else if code == Some(0) {
            self.push_log(
                LogStream::System,
                format!(">>> 任务完成 (exit {code_str})"),
            );
            self.status_text = format!("已完成 · exit {code_str}");
            self.last_failed = false;
        } else {
            self.push_log(
                LogStream::System,
                format!(">>> 任务异常结束 (exit {code_str})"),
            );
            self.status_text = format!("失败 · exit {code_str}");
            self.last_failed = true;
        }

        self.task = None;
        self.phase = TaskPhase::Idle;
        self.started_at = None;
        self.auto_scroll = true;
    }

    /// App 销毁前清理子进程。
    pub fn cleanup(&mut self) {
        if let Some(task) = self.task.take() {
            task.force_cleanup();
        }
        self.phase = TaskPhase::Idle;
    }

    /// 运行中 spinner 字符。
    pub fn spinner(&self) -> char {
        const FRAMES: [char; 4] = ['⠋', '⠙', '⠹', '⠸'];
        FRAMES[(self.frame_tick as usize / 2) % FRAMES.len()]
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
