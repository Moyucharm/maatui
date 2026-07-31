//! TUI 渲染：分层菜单、配置编辑、自动战斗、日志与弹窗。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::app::{
    App, EditorSection, MainMenuItem, Screen, SelectDialog, TASK_TYPES, TaskPhase, variant_fields,
};
use crate::runner::LogLevel;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let show_logs =
        app.phase != TaskPhase::Idle || matches!(app.screen, Screen::Daily | Screen::Copilot);
    let chunks = if show_logs {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                if app.phase == TaskPhase::Idle {
                    Constraint::Min(8)
                } else {
                    Constraint::Length(4)
                },
                Constraint::Min(8),
                Constraint::Length(1),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(1),
            ])
            .split(area)
    };

    draw_header(frame, app, chunks[0]);
    if app.phase == TaskPhase::Idle {
        draw_screen(frame, app, chunks[1]);
    } else {
        draw_running_control(frame, app, chunks[1]);
    }
    if show_logs {
        draw_logs(frame, app, chunks[2]);
        draw_footer(frame, app, chunks[3]);
    } else {
        draw_footer(frame, app, chunks[2]);
    }

    if let Some(input) = &app.input {
        draw_input_dialog(frame, area, &input.title, &input.value);
    }
    if let Some(select) = &app.select {
        draw_select_dialog(frame, area, select);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm_dialog(frame, area, &confirm.message);
    }
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
    frame.render_widget(
        Paragraph::new(title).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(MUTED)),
        ),
        area,
    );
}

fn draw_screen(frame: &mut Frame, app: &App, area: Rect) {
    match app.screen {
        Screen::Main => draw_main(frame, app, area),
        Screen::Daily => draw_daily(frame, app, area),
        Screen::Config => draw_config(frame, app, area),
        Screen::AddTask => draw_add_task(frame, app, area),
        Screen::TaskEdit => draw_task_edit(frame, app, area),
        Screen::VariantList => draw_variant_list(frame, app, area),
        Screen::VariantEdit => draw_variant_edit(frame, app, area),
        Screen::Copilot => draw_copilot(frame, app, area),
    }
}

fn draw_main(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = MainMenuItem::ALL
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let selected = index == app.main_idx;
            let style = if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(if selected { " ▶ " } else { "   " }, style),
                Span::styled(pad_display_width(item.label(), 12), style),
                Span::styled(item.hint(), Style::default().fg(MUTED)),
            ]))
        })
        .collect();
    render_list(frame, area, " 主菜单 ", items, app.main_idx);
}

fn draw_daily(frame: &mut Frame, app: &App, area: Rect) {
    let task_count = app.config.as_ref().map_or(0, |config| config.len());
    let config_status = app
        .config
        .as_ref()
        .map(|config| config.path().display().to_string())
        .unwrap_or_else(|| "daily 配置未加载".to_string());
    let rows = [
        ("开始运行", "maa run daily -v".to_string()),
        ("配置管理", format!("{task_count} 个任务 · {config_status}")),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, (label, value))| field_item(index == app.daily_idx, label, value))
        .collect();
    render_list(frame, area, " 每日任务 · 独立运行页 ", items, app.daily_idx);
}

fn draw_config(frame: &mut Frame, app: &App, area: Rect) {
    let Some(config) = &app.config else {
        let message = app.config_error.as_deref().unwrap_or("daily 配置未加载");
        frame.render_widget(
            Paragraph::new(format!("\n  × {message}\n\n  按 r 重试，Esc 返回"))
                .style(Style::default().fg(ERR))
                .block(panel(" 配置管理 ")),
            area,
        );
        return;
    };

    let items: Vec<ListItem> = (0..config.len())
        .filter_map(|index| config.task_summary(index).map(|summary| (index, summary)))
        .map(|(index, summary)| {
            let selected = index == app.config_idx;
            let enabled = if summary.enabled { "[开]" } else { "[关]" };
            let enabled_color = if summary.enabled { OK } else { MUTED };
            let style = if selected {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "▶ " } else { "  " }, style),
                Span::styled(format!("{enabled:<5}"), Style::default().fg(enabled_color)),
                Span::styled(pad_display_width(&summary.name, 18), style),
                Span::styled(summary.task_type, Style::default().fg(MUTED)),
            ]))
        })
        .collect();
    let title = format!(" daily 配置 · {} ", config.path().display());
    render_list(frame, area, &title, items, app.config_idx);
}

fn draw_add_task(frame: &mut Frame, app: &App, area: Rect) {
    let items = TASK_TYPES
        .iter()
        .enumerate()
        .map(|(index, task_type)| {
            let selected = index == app.add_task_idx;
            ListItem::new(Span::styled(
                format!("{} {task_type}", if selected { "▶" } else { " " }),
                if selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ))
        })
        .collect();
    render_list(frame, area, " 新增任务类型 ", items, app.add_task_idx);
}

fn draw_task_edit(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);
    let tabs = Line::from(
        EditorSection::ALL
            .iter()
            .enumerate()
            .flat_map(|(index, section)| {
                let selected = index == app.section_idx;
                [
                    Span::raw("  "),
                    Span::styled(
                        section.label(),
                        if selected {
                            Style::default()
                                .fg(ACCENT)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                        } else {
                            Style::default().fg(MUTED)
                        },
                    ),
                ]
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(Paragraph::new(tabs).block(panel(" 任务编辑 ")), chunks[0]);

    if app.editor_section() == EditorSection::Variants {
        let count = app
            .config
            .as_ref()
            .map_or(0, |config| config.variant_count(app.config_idx));
        frame.render_widget(
            Paragraph::new(format!(
                "\n  当前任务包含 {count} 个条件变体。\n\n  按 Enter 进入变体列表；复杂条件放在此三级菜单中管理。"
            ))
            .style(Style::default().fg(Color::White))
            .block(panel(" 条件与变体 ")),
            chunks[1],
        );
        return;
    }

    let fields = app.current_task_fields();
    let items: Vec<ListItem> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == app.field_idx;
            let value = app
                .task_field_value(field)
                .unwrap_or_else(|| field.default.clone())
                .display();
            field_item(selected, field.label, &value)
        })
        .collect();
    render_list(
        frame,
        chunks[1],
        " 字段（Enter 编辑，布尔值直接切换） ",
        items,
        app.field_idx,
    );
}

fn draw_variant_list(frame: &mut Frame, app: &App, area: Rect) {
    let items = app
        .config
        .as_ref()
        .map(|config| {
            (0..config.variant_count(app.config_idx))
                .filter_map(|index| {
                    config
                        .variant_summary(app.config_idx, index)
                        .map(|summary| (index, summary))
                })
                .map(|(index, summary)| {
                    let selected = index == app.variant_idx;
                    ListItem::new(Span::styled(
                        format!(
                            "{} 变体 {} · {summary}",
                            if selected { "▶" } else { " " },
                            index + 1
                        ),
                        if selected {
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    render_list(
        frame,
        area,
        " 条件变体 · a 新增 / d 删除 / Shift+↑↓ 移动 ",
        items,
        app.variant_idx,
    );
}

fn draw_variant_edit(frame: &mut Frame, app: &App, area: Rect) {
    let fields = variant_fields();
    let items = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == app.field_idx;
            let value = app
                .variant_field_value(field)
                .unwrap_or_else(|| field.default.clone())
                .display();
            field_item(selected, field.label, &value)
        })
        .collect();
    render_list(
        frame,
        area,
        " 编辑变体 · 条件与覆盖参数 ",
        items,
        app.field_idx,
    );
}

fn draw_copilot(frame: &mut Frame, app: &App, area: Rect) {
    let rows = [
        (
            "作业来源",
            if app.copilot.source.is_empty() {
                "<请输入>".to_string()
            } else {
                app.copilot.source.clone()
            },
        ),
        ("模式", app.copilot.raid.label().to_string()),
        ("自动编队", on_off(app.copilot.formation)),
        (
            "编队编号",
            if app.copilot.formation_index == 0 {
                "当前".to_string()
            } else {
                app.copilot.formation_index.to_string()
            },
        ),
        ("使用理智药", on_off(app.copilot.use_sanity_potion)),
        ("补低信赖干员", on_off(app.copilot.add_trust)),
        ("忽略练度要求", on_off(app.copilot.ignore_requirements)),
        (
            "助战模式",
            copilot_support_label(app.copilot.support_unit_usage),
        ),
        (
            "指定助战",
            if app.copilot.support_unit_name.is_empty() {
                "<无>".to_string()
            } else {
                app.copilot.support_unit_name.clone()
            },
        ),
        ("循环次数", app.copilot.loop_times.to_string()),
        ("开始运行", "maa copilot ... -v".to_string()),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, (label, value))| field_item(index == app.copilot_idx, label, value))
        .collect();
    render_list(
        frame,
        area,
        " 自动战斗 · 支持数字 / maa:// / prts:// / 本地 JSON ",
        items,
        app.copilot_idx,
    );
}

fn draw_running_control(frame: &mut Frame, app: &App, area: Rect) {
    let label = match app.phase {
        TaskPhase::Running => format!("{}  {} 运行中", app.spinner(), app.active_label),
        TaskPhase::Stopping => format!("{}停止中…", app.active_label),
        TaskPhase::Idle => String::new(),
    };
    let color = if app.phase == TaskPhase::Stopping {
        WARN
    } else {
        OK
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("  {label:<32}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "[ Enter / s ] 停止任务",
                Style::default().fg(ERR).add_modifier(Modifier::BOLD),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color))
                .title(Span::styled(" 运行控制 ", Style::default().fg(ACCENT))),
        ),
        area,
    );
}

fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = panel(" 日志 ").title_bottom(Line::from(Span::styled(
        format!(
            " {} · {} 行 ",
            if app.auto_scroll { "AUTO" } else { "MANUAL" },
            app.logs.len()
        ),
        Style::default().fg(MUTED),
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let max_scroll = app.max_scroll(inner.height);
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
            let (prefix, color) = log_style(log.level);
            Line::from(vec![
                Span::styled(
                    format!("{prefix} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(log.text.clone(), Style::default().fg(color)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        inner,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let keys = if app.phase != TaskPhase::Idle {
        "Enter/s 停止  PgUp/PgDn 滚动  q 停止并退出"
    } else {
        match app.screen {
            Screen::Main => "↑↓/jk 选择  Enter 确认  q 退出",
            Screen::Daily => "↑↓/jk 选择  Enter 运行/配置  r 运行  c 配置  Esc 返回",
            Screen::Config => {
                "Space 开关  Enter/e 编辑  a 新增  d 删除  Shift+↑↓ 移动  r 重载  Esc 返回"
            }
            Screen::AddTask => "↑↓ 选择类型  Enter 新增并编辑  Esc 返回",
            Screen::TaskEdit => "←→/hl 切换层级  ↑↓ 选择  Enter/e 编辑  v 创建活动变体  Esc 返回",
            Screen::VariantList => "a 新增  d 删除  Shift+↑↓ 移动  Enter 编辑  Esc 返回",
            Screen::VariantEdit => "↑↓ 选择  Enter 编辑  Esc 返回",
            Screen::Copilot => "↑↓ 选择  Enter/e 编辑或切换  r 运行  Esc 返回",
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {keys}"), Style::default().fg(MUTED))),
        area,
    );
}

fn draw_input_dialog(frame: &mut Frame, area: Rect, title: &str, value: &str) {
    let popup = centered_rect(76, 7, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "  > ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    value,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::styled("█", Style::default().fg(ACCENT)),
            ]),
            Line::from(Span::styled(
                "  Enter 确认 · Esc 取消",
                Style::default().fg(MUTED),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(format!(" {title} ")),
        ),
        popup,
    );
}

fn draw_select_dialog(frame: &mut Frame, area: Rect, select: &SelectDialog) {
    let height = ((select.options.len() as u16).saturating_add(7))
        .clamp(9, area.height.saturating_sub(2).max(9));
    let popup = centered_rect(84, height, area);
    frame.render_widget(Clear, popup);

    let title = format!(" {} ", select.title);
    let outer = panel(&title);
    let inner = outer.inner(popup);
    frame.render_widget(outer, popup);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(inner);

    let items: Vec<ListItem> = if select.options.is_empty() {
        vec![ListItem::new(Span::styled(
            "暂无可用选项",
            Style::default().fg(WARN),
        ))]
    } else {
        select
            .options
            .iter()
            .map(|option| {
                let marker = if select.multi {
                    if select.selected_values.contains(&option.value) {
                        "[x]"
                    } else {
                        "[ ]"
                    }
                } else {
                    "   "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{marker} "), Style::default().fg(MUTED)),
                    Span::styled(&option.label, Style::default().fg(Color::White)),
                    Span::styled(
                        format!("  [{}]", option.value.display()),
                        Style::default().fg(MUTED),
                    ),
                ]))
            })
            .collect()
    };
    let mut state = ListState::default();
    if !select.options.is_empty() {
        state.select(Some(select.selected.min(select.options.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items).highlight_symbol("▶ "),
        chunks[0],
        &mut state,
    );

    let description = select
        .options
        .get(select.selected)
        .map_or("可用 ↑↓ 选择".to_string(), |option| {
            option.description.clone()
        });
    let custom_hint = match (select.allow_custom, select.refreshable, select.multi) {
        (true, true, _) => " · c 自定义 · r 刷新目录",
        (true, false, _) => " · c 自定义",
        (false, true, _) => " · r 刷新目录",
        (false, false, true) => " · Space 勾选",
        (false, false, false) => "",
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}\nEnter 确认 · Esc 取消",
            description, custom_hint
        ))
        .style(Style::default().fg(MUTED))
        .wrap(Wrap { trim: true }),
        chunks[1],
    );
}

fn draw_confirm_dialog(frame: &mut Frame, area: Rect, message: &str) {
    let popup = centered_rect(64, 7, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(message, Style::default().fg(WARN)))
                .alignment(Alignment::Center),
            Line::from(Span::styled(
                "Enter/y 确认 · n/Esc 取消",
                Style::default().fg(MUTED),
            ))
            .alignment(Alignment::Center),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(WARN))
                .title(" 确认操作 "),
        ),
        popup,
    );
}

fn render_list(frame: &mut Frame, area: Rect, title: &str, items: Vec<ListItem>, selected: usize) {
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(selected.min(items.len() - 1)));
    }
    frame.render_stateful_widget(List::new(items).block(panel(title)), area, &mut state);
}

const FIELD_LABEL_WIDTH: usize = 20;

fn field_item(selected: bool, label: &str, value: &str) -> ListItem<'static> {
    let label_style = if selected {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    ListItem::new(Line::from(vec![
        Span::styled(if selected { "▶ " } else { "  " }, label_style),
        Span::styled(pad_display_width(label, FIELD_LABEL_WIDTH), label_style),
        Span::styled(
            value.to_string(),
            Style::default().fg(if selected { Color::White } else { MUTED }),
        ),
    ]))
}

fn pad_display_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(padding))
}

fn panel<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .title(Span::styled(title, Style::default().fg(ACCENT)))
}

fn log_style(level: LogLevel) -> (&'static str, Color) {
    match level {
        LogLevel::Plain => ("│", Color::White),
        LogLevel::Info => ("i", ACCENT),
        LogLevel::Success => ("✓", OK),
        LogLevel::Warn => ("!", WARN),
        LogLevel::Error => ("×", ERR),
        LogLevel::Debug => ("·", Color::Gray),
        LogLevel::Trace => ("·", MUTED),
        LogLevel::System => ("◆", ACCENT),
    }
}

fn on_off(value: bool) -> String {
    if value { "开" } else { "关" }.to_string()
}

fn copilot_support_label(value: i64) -> String {
    match value {
        0 => "不使用助战".to_string(),
        1 => "仅补一名缺失".to_string(),
        2 => "缺失时补齐，否则指定".to_string(),
        3 => "缺失时补齐，否则随机".to_string(),
        other => format!("未知模式 ({other})"),
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
