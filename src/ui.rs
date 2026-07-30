//! TUI 渲染：菜单、日志、状态栏。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, MenuItem, TaskPhase};
use crate::runner::LogStream;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // 顶栏
            Constraint::Length(5), // 菜单
            Constraint::Min(5),    // 日志
            Constraint::Length(1), // 底栏
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_menu(frame, app, chunks[1]);
    draw_logs(frame, app, chunks[2]);
    draw_footer(frame, app, chunks[3]);
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let (dot, phase_label, phase_color) = match app.phase {
        TaskPhase::Idle if app.last_failed => ("●", "Failed", ERR),
        TaskPhase::Idle => ("●", "Idle", MUTED),
        TaskPhase::Running => ("●", "Running", OK),
        TaskPhase::Stopping => ("●", "Stopping", WARN),
    };

    let title = Line::from(vec![
        Span::styled(
            " MaaTUI ",
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(dot, Style::default().fg(phase_color)),
        Span::raw(" "),
        Span::styled(phase_label, Style::default().fg(phase_color)),
        Span::raw("  ·  "),
        Span::styled(&app.status_text, Style::default().fg(Color::Gray)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED));
    let para = Paragraph::new(title).block(block);
    frame.render_widget(para, area);
}

fn draw_menu(frame: &mut Frame, app: &App, area: Rect) {
    let running = app.phase != TaskPhase::Idle;

    let items: Vec<ListItem> = MenuItem::ALL
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let selected = i == app.menu_idx;
            let marker = if selected { "▶" } else { " " };

            let mut label = item.label().to_string();
            if *item == MenuItem::Daily && app.phase == TaskPhase::Running {
                label = format!("{}  {} 运行中", label, app.spinner());
            } else if *item == MenuItem::Daily && app.phase == TaskPhase::Stopping {
                label = format!("{}  停止中…", label);
            }

            let label_style = if selected && !running {
                Style::default()
                    .fg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if selected && running && *item == MenuItem::Daily {
                Style::default().fg(OK).add_modifier(Modifier::BOLD)
            } else if running {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(Color::White)
            };

            let hint_style = Style::default().fg(MUTED);

            let line = Line::from(vec![
                Span::styled(format!(" {marker} "), label_style),
                Span::styled(format!("{label:<12}"), label_style),
                Span::styled(item.hint(), hint_style),
            ]);
            ListItem::new(line)
        })
        .collect();

    let title = if running {
        " 操作（运行中 · 菜单锁定） "
    } else {
        " 操作 "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if running { OK } else { MUTED }))
        .title(Span::styled(title, Style::default().fg(ACCENT)));

    let list = List::new(items).block(block);
    let mut state = ListState::default();
    state.select(Some(app.menu_idx));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .title(Span::styled(" 日志 ", Style::default().fg(ACCENT)))
        .title_bottom(log_scroll_hint(app));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = inner.height;
    let max_scroll = app.max_scroll(visible);

    if app.auto_scroll {
        app.scroll = max_scroll;
    } else {
        app.scroll = app.scroll.min(max_scroll);
        if app.scroll >= max_scroll {
            app.auto_scroll = true;
        }
    }

    let lines: Vec<Line> = app
        .logs
        .iter()
        .map(|log| {
            let (prefix, color) = match log.stream {
                LogStream::Stdout => ("│", Color::Gray),
                LogStream::Stderr => ("!", WARN),
                LogStream::System => ("*", ACCENT),
            };
            Line::from(vec![
                Span::styled(format!("{prefix} "), Style::default().fg(color)),
                Span::styled(
                    log.text.clone(),
                    Style::default().fg(match log.stream {
                        LogStream::System => ACCENT,
                        LogStream::Stderr => WARN,
                        LogStream::Stdout => Color::White,
                    }),
                ),
            ])
        })
        .collect();

    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(para, inner);
}

fn log_scroll_hint(app: &App) -> Line<'static> {
    let mode = if app.auto_scroll {
        "AUTO"
    } else {
        "MANUAL"
    };
    Line::from(vec![
        Span::styled(
            format!(" {mode} · {} 行 ", app.logs.len()),
            Style::default().fg(MUTED),
        ),
    ])
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let keys = match app.phase {
        TaskPhase::Idle => "↑↓/jk 选择  Enter 确认  PgUp/PgDn 滚动  q 退出",
        TaskPhase::Running => "s 停止  PgUp/PgDn 滚动  q 停止并退出",
        TaskPhase::Stopping => "等待进程退出…  q 强制退出流程",
    };

    let line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(keys, Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
