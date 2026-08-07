//! TUI 渲染：分层菜单、配置编辑、自动战斗、日志与弹窗。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    App, CopilotDetailDialog, CopilotSection, EditorSection, InputDialog, LogScope, MainMenuItem,
    Screen, SelectDialog, TASK_TYPES, TaskPhase, variant_fields,
};
use crate::runner::LogLevel;
use crate::shortcuts::{ShortcutHint, hints as shortcut_hints_for};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const ERR: Color = Color::Red;

// 日志使用固定 RGB，避免终端主题把 ANSI 青色重映射成黄色。
const LOG_TEXT: Color = Color::Rgb(205, 214, 244);
const LOG_INFO: Color = Color::Rgb(137, 220, 235);
const LOG_SUCCESS: Color = Color::Rgb(166, 227, 161);
const LOG_WARN: Color = Color::Rgb(249, 226, 175);
const LOG_ERROR: Color = Color::Rgb(243, 139, 168);
const LOG_DEBUG: Color = Color::Rgb(147, 153, 178);
const LOG_TRACE: Color = Color::Rgb(108, 112, 134);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let show_logs = app.phase != TaskPhase::Idle
        || matches!(app.screen, Screen::Daily | Screen::Copilot | Screen::Update);
    let chunks = if show_logs {
        let form_constraint = if app.phase == TaskPhase::Idle {
            Constraint::Length(form_content_height(app, area.height))
        } else {
            Constraint::Length(4)
        };
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                form_constraint,
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
        draw_input_dialog(frame, area, input);
    }
    if let Some(select) = &app.select {
        draw_select_dialog(frame, area, select);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm_dialog(frame, area, &confirm.message);
    }
    if let Some(detail) = app.copilot_detail.as_mut() {
        draw_copilot_detail(frame, area, detail);
    }
    if app.shortcut_help_open {
        draw_shortcut_help(frame, area, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let (dot, phase_label, phase_color) = match app.phase {
        TaskPhase::Idle if app.last_failed => ("●", "Failed", ERR),
        TaskPhase::Idle => ("●", "Idle", MUTED),
        TaskPhase::Running => ("●", "Running", OK),
        TaskPhase::Stopping => ("●", "Stopping", WARN),
    };
    let status_width = area.width.saturating_sub(28) as usize;
    let status_text = truncate_display_width(&app.status_text, status_width);
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
        Span::styled(status_text, Style::default().fg(Color::Gray)),
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
        Screen::Update => draw_update(frame, app, area),
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
    let config_status = if app.config.is_some() {
        "已加载"
    } else {
        "未加载"
    };
    let rows = [
        ("开始运行", "maa run daily -v".to_string()),
        ("配置管理", format!("{task_count} 个任务 · {config_status}")),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, (label, value))| field_item(index == app.daily_idx, label, value))
        .collect();
    render_list(frame, area, " 每日任务 ", items, app.daily_idx);
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
    render_list(frame, area, " 配置管理 ", items, app.config_idx);
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
                .unwrap_or_else(|| field.default.clone());
            field_item(selected, field.label, &app.field_display(field, &value))
        })
        .collect();
    render_list(frame, chunks[1], " 字段 ", items, app.field_idx);
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
    render_list(frame, area, " 条件变体 ", items, app.variant_idx);
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
                .unwrap_or_else(|| field.default.clone());
            field_item(selected, field.label, &app.field_display(field, &value))
        })
        .collect();
    render_list(frame, area, " 编辑变体 ", items, app.field_idx);
}

fn draw_copilot(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    let tabs = Line::from(
        CopilotSection::ALL
            .iter()
            .enumerate()
            .flat_map(|(index, section)| {
                let selected = index == app.copilot_section_idx;
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
    frame.render_widget(Paragraph::new(tabs).block(panel(" 自动战斗 ")), chunks[0]);

    match app.copilot_section() {
        CopilotSection::Singles => draw_single_copilot(frame, app, chunks[1]),
        CopilotSection::Sets => draw_copilot_list(frame, app, chunks[1]),
        CopilotSection::Settings => draw_copilot_settings(frame, app, chunks[1]),
    }
}

fn draw_single_copilot(frame: &mut Frame, app: &App, area: Rect) {
    let Some(cache) = &app.copilot_cache else {
        frame.render_widget(
            Paragraph::new("\n  × 作业缓存未加载")
                .style(Style::default().fg(ERR))
                .block(panel(" 当前单作业 ")),
            area,
        );
        return;
    };
    let lines = if let Some(entry) = cache.current_single() {
        let supported = app
            .current_single_supported_modes
            .map(|(normal, raid)| match (normal, raid) {
                (true, true) => "普通 / 突袭",
                (false, true) => "突袭",
                _ => "普通",
            })
            .unwrap_or("未知");
        let current = if entry.is_raid { "突袭" } else { "普通" };
        vec![
            Line::from(vec![
                Span::styled("  当前作业  ", Style::default().fg(MUTED)),
                Span::styled(entry.display_name(), Style::default().fg(Color::White)),
            ]),
            Line::from(format!("  关卡      {}", entry.stage_name)),
            Line::from(format!("  支持模式  {supported}")),
            Line::from(format!("  运行模式  {current}")),
            Line::from(format!("  来源      {}", entry.source_label())),
            Line::from(""),
            Line::from(Span::styled(
                "  运行前请手动打开对应准备界面；双模式作业可按 Space 切换运行模式。",
                Style::default().fg(WARN),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  尚未选择当前单作业",
                Style::default().fg(WARN),
            )),
            Line::from(Span::styled(
                "  按 e 搜索作业；编辑框内 Ctrl+U 可快速清空。",
                Style::default().fg(MUTED),
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(" 当前单作业 ")),
        area,
    );
}

fn draw_copilot_list(frame: &mut Frame, app: &App, area: Rect) {
    let Some(cache) = &app.copilot_cache else {
        let message = app.copilot_error.as_deref().unwrap_or("作业列表未加载");
        frame.render_widget(
            Paragraph::new(format!("\n  × {message}"))
                .style(Style::default().fg(ERR))
                .block(panel(" 作业列表 ")),
            area,
        );
        return;
    };

    let indices = app.copilot_visible_indices();
    let items: Vec<ListItem> = if indices.is_empty() {
        let (empty, hint) = ("暂无批量作业条目", "按 a 添加作业集或单个作业");
        vec![ListItem::new(Line::from(vec![
            Span::styled(format!("  {empty}"), Style::default().fg(WARN)),
            Span::styled(format!(" · {hint}"), Style::default().fg(MUTED)),
        ]))]
    } else {
        indices
            .iter()
            .enumerate()
            .filter_map(|(visible_index, &index)| {
                let entry = cache.entry(index)?;
                let selected = visible_index == app.copilot_idx;
                let style = if selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let enabled = if entry.enabled { "[开]" } else { "[关]" };
                let enabled_color = if entry.enabled { OK } else { MUTED };
                let difficulty = if entry.is_raid { "突袭" } else { "普通" };
                let difficulty_color = if entry.is_raid { WARN } else { LOG_INFO };
                let source = format!("{}  {}", entry.origin.label(), entry.source_label());
                Some(ListItem::new(Line::from(vec![
                    Span::styled(if selected { "▶ " } else { "  " }, style),
                    Span::styled(format!("{enabled:<5}"), Style::default().fg(enabled_color)),
                    Span::styled(
                        format!("{difficulty:<6}"),
                        Style::default().fg(difficulty_color),
                    ),
                    Span::styled(pad_display_width(&entry.stage_name, 12), style),
                    Span::styled(entry.display_name(), style),
                    Span::styled(format!("  {source}"), Style::default().fg(MUTED)),
                ])))
            })
            .collect()
    };
    render_list(frame, area, " 作业集 ", items, app.copilot_idx);
}

fn draw_copilot_settings(frame: &mut Frame, app: &App, area: Rect) {
    let rows = [
        (
            "自动编队",
            format!("{} · 批量模式强制开启", on_off(app.copilot.formation)),
        ),
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
        ("循环次数", format!("{} · 仅单作业", app.copilot.loop_times)),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, (label, value))| field_item(index == app.copilot_settings_idx, label, value))
        .collect();
    render_list(frame, area, " 运行设置 ", items, app.copilot_settings_idx);
}

fn draw_update(frame: &mut Frame, app: &App, area: Rect) {
    let options = ["仅更新活动与导航资源", "更新 MaaCore 与基础资源"];
    let details = [
        [
            "作用范围：活动关卡、导航与 MaaResource 热更新资源",
            "不会更新：MaaCore 与随包基础资源",
            "适用场景：活动开放、关卡导航或资源数据更新",
            "执行命令：maa hot-update --batch -v",
        ],
        [
            "作用范围：MaaCore 与随包基础资源",
            "同时更新：核心运行库及兼容性相关资源",
            "适用场景：OCR、核心兼容性或版本问题修复",
            "执行命令：maa update --batch -v",
        ],
    ];
    let selected = app.update_idx.min(options.len() - 1);
    let chunks = if area.width >= 80 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(5)])
            .split(area)
    };
    let items = options
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let is_selected = index == selected;
            ListItem::new(Span::styled(
                format!("{} {label}", if is_selected { "▶" } else { " " }),
                if is_selected {
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                },
            ))
        })
        .collect();
    render_list(frame, chunks[0], " 更新方式 ", items, selected);

    let lines = details[selected]
        .iter()
        .enumerate()
        .flat_map(|(index, text)| {
            let mut lines = vec![Line::from(format!("  {text}"))];
            if index + 1 < details[selected].len() {
                lines.push(Line::from(""));
            }
            lines
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .block(panel(" 说明 ")),
        chunks[1],
    );
}

fn draw_running_control(frame: &mut Frame, app: &App, area: Rect) {
    let label = match app.phase {
        TaskPhase::Running => {
            let progress = app.current_run_progress().map(|progress| {
                format!(
                    "当前进度：[{}/{}] {}",
                    progress.current, progress.total, progress.label
                )
            });
            if let Some(progress) = progress {
                format!("{}  {}", app.spinner(), progress)
            } else if let Some((current, total, name)) = app.copilot_batch_progress() {
                format!(
                    "{}  {}  [{current}/{total}] {name}",
                    app.spinner(),
                    app.active_label
                )
            } else {
                format!("{}  {} 运行中", app.spinner(), app.active_label)
            }
        }
        TaskPhase::Stopping => format!("{}停止中…", app.active_label),
        TaskPhase::Idle => String::new(),
    };
    let color = if app.phase == TaskPhase::Stopping {
        WARN
    } else {
        OK
    };
    let stop = "[ Enter / s ] 停止任务";
    let inner_width = area.width.saturating_sub(2) as usize;
    let progress_width = inner_width.saturating_sub(stop.width());
    let progress = if progress_width > 2 {
        let content = truncate_display_width(&label, progress_width - 2);
        pad_display_width(&format!("  {content}"), progress_width)
    } else {
        " ".repeat(progress_width)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                progress,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(stop, Style::default().fg(ERR).add_modifier(Modifier::BOLD)),
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
    let scope = app.visible_log_scope();
    let buffer = app.log_buffer(scope);
    let title = match scope {
        LogScope::Daily => " 每日任务日志 ",
        LogScope::Copilot => " 自动战斗日志 ",
        LogScope::Update => " 更新管理日志 ",
    };
    let block = panel(title).title_bottom(Line::from(Span::styled(
        format!(
            " {} · {} 行 ",
            if buffer.auto_scroll { "AUTO" } else { "MANUAL" },
            buffer.lines.len()
        ),
        Style::default().fg(MUTED),
    )));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let buffer = app.log_buffer_mut(scope);
    let max_scroll = (buffer.lines.len() as u16).saturating_sub(inner.height.max(1));
    if buffer.auto_scroll {
        buffer.scroll = max_scroll;
    } else {
        buffer.scroll = buffer.scroll.min(max_scroll);
        if buffer.scroll >= max_scroll {
            buffer.auto_scroll = true;
        }
    }

    let lines: Vec<Line> = buffer
        .lines
        .iter()
        .map(|log| {
            let (prefix, level_color, text_color) = log_style(log.level);
            Line::from(vec![
                Span::styled(
                    format!("{prefix} "),
                    Style::default()
                        .fg(level_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(log.text.clone(), Style::default().fg(text_color)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((buffer.scroll, 0)),
        inner,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let available = area.width.saturating_sub(1) as usize;
    let keys = footer_keys(app, available);
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {keys}"), Style::default().fg(MUTED))),
        area,
    );
}

fn footer_keys(app: &App, width: usize) -> String {
    if let Some(keys) = modal_footer_keys(app) {
        return truncate_display_width(keys, width);
    }

    const HELP: &str = "?全部";
    if width < HELP.width() {
        return truncate_display_width(HELP, width);
    }

    let mut hints = shortcut_hints(app);
    hints.sort_by_key(|hint| hint.priority);
    let mut parts = Vec::new();
    let mut used = HELP.width();
    for hint in hints {
        let part = format!("{}{}", hint.key, hint.label);
        let cost = part.width() + 2;
        if used + cost <= width {
            used += cost;
            parts.push(part);
        }
    }
    parts.push(HELP.to_string());
    parts.join("  ")
}

fn modal_footer_keys(app: &App) -> Option<&'static str> {
    if app.shortcut_help_open {
        Some("↑↓/jk滚动  PgUp/PgDn快翻  ?/Esc/q关闭")
    } else if app.copilot_detail.is_some() {
        Some("↑↓/jk滚动  PgUp/PgDn快翻  Esc/q关闭")
    } else if app.input.is_some() {
        Some("Enter确认  Esc取消  Ctrl+U清空")
    } else if app.select.is_some() {
        Some("↑↓/jk选择  Enter确认  Esc取消")
    } else if app.confirm.is_some() {
        Some("Enter/y确认  n/Esc取消")
    } else {
        None
    }
}

fn shortcut_hints(app: &App) -> Vec<ShortcutHint> {
    shortcut_hints_for(app.shortcut_context())
}

fn shortcut_help_title(app: &App) -> String {
    if app.phase != TaskPhase::Idle {
        return "任务运行中".to_string();
    }
    match app.screen {
        Screen::Main => "主菜单".to_string(),
        Screen::Daily => "每日任务".to_string(),
        Screen::Config => "配置管理".to_string(),
        Screen::AddTask => "新增任务".to_string(),
        Screen::TaskEdit => "任务编辑".to_string(),
        Screen::VariantList => "变体列表".to_string(),
        Screen::VariantEdit => "变体编辑".to_string(),
        Screen::Copilot => format!("自动战斗 · {}", app.copilot_section().label()),
        Screen::Update => "更新管理".to_string(),
    }
}

fn input_dialog_hint(input: &InputDialog) -> &'static str {
    if input.allows_import_kind_switch() {
        "  Enter 确认 · Esc 取消 · Ctrl+U 清空 · ←/→ 切换添加类型"
    } else {
        "  Enter 确认 · Esc 取消 · Ctrl+U 清空"
    }
}

fn draw_input_dialog(frame: &mut Frame, area: Rect, input: &InputDialog) {
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
                    &input.value,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::UNDERLINED),
                ),
                Span::styled("█", Style::default().fg(ACCENT)),
            ]),
            Line::from(Span::styled(
                input_dialog_hint(input),
                Style::default().fg(MUTED),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(format!(" {} ", input.title)),
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

fn draw_copilot_detail(frame: &mut Frame, area: Rect, detail: &mut CopilotDetailDialog) {
    let height = area.height.saturating_sub(4).clamp(10, 30);
    let popup = centered_rect(92, height, area);
    frame.render_widget(Clear, popup);
    let title = format!(" {} ", detail.title);
    let block = panel(&title).title_bottom(Line::from(Span::styled(
        " Esc/q 关闭 · ↑↓/jk 滚动 · PgUp/PgDn 快速滚动 ",
        Style::default().fg(MUTED),
    )));
    let inner = block.inner(popup);
    let popup_width = inner.width.max(1);
    let content_rows = detail
        .lines
        .iter()
        .map(|line| {
            let width = line.width() as u16;
            width.max(1).div_ceil(popup_width)
        })
        .sum::<u16>();
    let max_scroll = content_rows.saturating_sub(inner.height);
    detail.scroll = detail.scroll.min(max_scroll);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(detail.lines.join("\n"))
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll, 0)),
        inner,
    );
}

fn draw_shortcut_help(frame: &mut Frame, area: Rect, app: &mut App) {
    let height = area.height.saturating_sub(4).clamp(10, 30);
    let popup = centered_rect(92, height, area);
    frame.render_widget(Clear, popup);
    let title = format!(" 快捷键 · {} ", shortcut_help_title(app));
    let block = panel(&title).title_bottom(Line::from(Span::styled(
        " ?/Esc/q 关闭 · ↑↓/jk 滚动 · PgUp/PgDn 快速滚动 ",
        Style::default().fg(MUTED),
    )));
    let inner = block.inner(popup);
    let hints = shortcut_hints(app);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for group in ["导航", "运行", "编辑", "批量"] {
        let group_hints: Vec<_> = hints.iter().filter(|hint| hint.group == group).collect();
        if group_hints.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(
            format!(" {group} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )));
        for hint in group_hints {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}", pad_display_width(hint.key, 16)),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    pad_display_width(hint.label, 12),
                    Style::default().fg(Color::White),
                ),
                Span::styled(hint.description, Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled(
            format!("  {}", pad_display_width("?", 16)),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            pad_display_width("全部快捷键", 12),
            Style::default().fg(Color::White),
        ),
        Span::styled("打开或关闭此帮助面板", Style::default().fg(MUTED)),
    ]));

    let popup_width = inner.width.max(1);
    let content_rows = lines
        .iter()
        .map(|line| (line.width() as u16).max(1).div_ceil(popup_width))
        .sum::<u16>();
    let max_scroll = content_rows.saturating_sub(inner.height);
    app.shortcut_help_scroll = app.shortcut_help_scroll.min(max_scroll);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.shortcut_help_scroll, 0)),
        inner,
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

fn truncate_display_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let target = max_width.saturating_sub(1);
    let mut result = String::new();
    let mut width = 0;
    for character in text.chars() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > target {
            break;
        }
        result.push(character);
        width += character_width;
    }
    result.push('…');
    result
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

/// Idle + 日志页的表单高度：内容行 + 上下边框，并给日志至少留 8 行。
fn form_content_height(app: &App, total_height: u16) -> u16 {
    const HEADER: u16 = 3;
    const FOOTER: u16 = 1;
    const LOGS_MIN: u16 = 8;
    const FORM_MIN: u16 = 4;
    const BORDER: u16 = 2;

    if app.screen == Screen::Update {
        let usable = total_height.saturating_sub(HEADER + FOOTER);
        let desired = usable.saturating_mul(2) / 3;
        return desired.clamp(FORM_MIN, usable.saturating_sub(LOGS_MIN).max(FORM_MIN));
    }

    let rows = match app.screen {
        Screen::Copilot => match app.copilot_section() {
            CopilotSection::Singles => 8,
            CopilotSection::Sets => app.copilot_visible_indices().len().max(1) as u16 + 3,
            CopilotSection::Settings => 11,
        },
        Screen::Daily => 2,
        Screen::Main => MainMenuItem::ALL.len() as u16,
        Screen::AddTask => TASK_TYPES.len() as u16,
        Screen::Config => app
            .config
            .as_ref()
            .map(|config| config.len().max(1) as u16)
            .unwrap_or(1),
        Screen::TaskEdit | Screen::VariantEdit => 6,
        Screen::VariantList => 4,
        Screen::Update => unreachable!("更新页已使用专用布局"),
    };
    let desired = rows.saturating_add(BORDER).max(FORM_MIN);
    let reserved = HEADER.saturating_add(FOOTER).saturating_add(LOGS_MIN);
    let available = total_height.saturating_sub(reserved).max(FORM_MIN);
    desired.min(available)
}

fn log_style(level: LogLevel) -> (&'static str, Color, Color) {
    match level {
        LogLevel::Plain => ("│", LOG_TRACE, LOG_TEXT),
        LogLevel::Info => ("i", LOG_INFO, LOG_TEXT),
        LogLevel::Success => ("✓", LOG_SUCCESS, LOG_TEXT),
        LogLevel::Warn => ("!", LOG_WARN, LOG_WARN),
        LogLevel::Error => ("×", LOG_ERROR, LOG_ERROR),
        LogLevel::Debug => ("·", LOG_DEBUG, LOG_DEBUG),
        LogLevel::Trace => ("·", LOG_TRACE, LOG_TRACE),
        LogLevel::System => ("◆", LOG_INFO, LOG_TEXT),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CopilotTextField, ImportDestination, InputDialog, InputTarget};
    use crate::copilot::ImportKind;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn info_log_uses_cyan_marker_and_neutral_text() {
        let (prefix, level_color, text_color) = log_style(LogLevel::Info);

        assert_eq!(prefix, "i");
        assert_eq!(level_color, LOG_INFO);
        assert_eq!(text_color, LOG_TEXT);
        assert_ne!(level_color, text_color);
    }

    #[test]
    fn warning_and_error_logs_keep_semantic_colors() {
        assert_eq!(log_style(LogLevel::Warn), ("!", LOG_WARN, LOG_WARN));
        assert_eq!(log_style(LogLevel::Error), ("×", LOG_ERROR, LOG_ERROR));
    }

    #[test]
    fn truncates_mixed_width_text_without_exceeding_limit() {
        assert_eq!(truncate_display_width("TO-1 作业详情", 8), "TO-1 作…");
        assert_eq!(truncate_display_width("short", 8), "short");
        assert_eq!(truncate_display_width("内容", 1), "…");
        assert!(truncate_display_width("长标题abcdef", 7).width() <= 7);
    }

    #[test]
    fn page_footer_exposes_help_when_space_allows() {
        let mut app = App::new();
        app.copilot_cache = None;
        app.screen = Screen::Copilot;
        app.copilot_section_idx = 1;
        for width in [5, 8, 16, 32, 60, 120] {
            let text = footer_keys(&app, width);
            assert!(text.width() <= width, "width={width}, text={text}");
            assert!(text.contains("?全部"), "width={width}, text={text}");
        }
        let narrow = footer_keys(&app, 4);
        assert!(narrow.width() <= 4);
        assert!(!narrow.contains("?全部"));
    }

    #[test]
    fn copilot_footer_keeps_labels_instead_of_key_only_fallback() {
        let mut app = App::new();
        app.copilot_cache = None;
        app.screen = Screen::Copilot;
        app.copilot_section_idx = 0;

        let text = footer_keys(&app, 32);

        assert!(text.contains("搜索"));
        assert!(text.contains("?全部"));
        assert!(!text.contains("e Enter Tab Esc"));
    }

    #[test]
    fn empty_copilot_set_keeps_all_shortcuts_discoverable() {
        let mut app = App::new();
        app.copilot_cache = None;
        app.screen = Screen::Copilot;
        app.copilot_section_idx = 1;

        let footer = footer_keys(&app, 160);
        let hints = shortcut_hints(&app);

        assert!(footer.contains("单跑"));
        assert!(footer.contains("批量"));
        assert!(footer.contains("?全部"));
        for key in ["Enter/e", "r", "Space", "a", "i", "d", "t", "c"] {
            assert!(hints.iter().any(|hint| hint.key == key), "missing {key}");
        }
    }

    #[test]
    fn modal_footer_uses_its_own_context_without_page_help() {
        let mut app = App::new();
        app.input = Some(InputDialog {
            title: "输入".to_string(),
            value: String::new(),
            target: InputTarget::CopilotText(CopilotTextField::SupportName),
        });

        let input_footer = footer_keys(&app, 80);
        assert!(input_footer.contains("Enter确认"));
        assert!(!input_footer.contains("?全部"));

        app.input = None;
        app.shortcut_help_open = true;
        let help_footer = footer_keys(&app, 80);
        assert!(help_footer.contains("关闭"));
        assert!(!help_footer.contains("?全部"));
    }

    #[test]
    fn input_dialog_only_shows_kind_switch_for_batch_imports() {
        let normal = InputDialog {
            title: "助战干员".to_string(),
            value: String::new(),
            target: InputTarget::CopilotText(CopilotTextField::SupportName),
        };
        let batch = InputDialog {
            title: "添加作业".to_string(),
            value: String::new(),
            target: InputTarget::CopilotAdd {
                kind: ImportKind::Set,
                destination: ImportDestination::Batch,
            },
        };

        assert!(!input_dialog_hint(&normal).contains("切换添加类型"));
        assert!(input_dialog_hint(&batch).contains("切换添加类型"));
    }

    #[test]
    fn footer_renders_in_a_narrow_test_backend() {
        let app = App::new();
        let backend = TestBackend::new(5, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_footer(frame, &app, frame.area()))
            .unwrap();

        assert_eq!(terminal.backend().buffer().area, Rect::new(0, 0, 5, 1));
    }

    #[test]
    fn update_layout_separates_options_from_selected_description() {
        let app = App::new();
        let backend = TestBackend::new(100, 16);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_update(frame, &app, frame.area()))
            .unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .filter(|symbol| !symbol.trim().is_empty())
            .collect::<String>();
        assert!(text.contains("更新方式"));
        assert!(text.contains("说明"));
        assert!(text.contains("仅更新活动与导航资源"));
        assert!(text.contains("执行命令：maahot-update--batch-v"));
    }

    #[test]
    fn update_layout_falls_back_for_narrow_terminals() {
        let app = App::new();
        let backend = TestBackend::new(60, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_update(frame, &app, frame.area()))
            .unwrap();

        assert_eq!(terminal.backend().buffer().area, Rect::new(0, 0, 60, 12));
    }

    #[test]
    fn idle_copilot_form_fits_content_and_leaves_logs_room() {
        let mut app = App::new();
        app.copilot_cache = None;
        app.screen = Screen::Copilot;
        // 单作业卡片为固定高度。
        assert_eq!(form_content_height(&app, 40), 10);
        app.copilot_section_idx = 1;
        assert_eq!(form_content_height(&app, 40), 6);
        app.copilot_section_idx = 2;
        assert_eq!(form_content_height(&app, 40), 13);
        // 矮终端时优先保住日志 Min(8)
        assert_eq!(form_content_height(&app, 20), 8);
        app.screen = Screen::Update;
        assert_eq!(form_content_height(&app, 40), 24);
        assert_eq!(form_content_height(&app, 20), 8);
    }
}
