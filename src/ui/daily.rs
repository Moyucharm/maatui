//! Daily 配置页面渲染。

use super::text::*;
use super::*;

pub(super) fn draw_daily(frame: &mut Frame, app: &App, area: Rect) {
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
        .map(|(index, (label, value))| field_item(index == app.daily_idx, label, value, area.width))
        .collect();
    render_list(frame, area, " 每日任务 ", items, app.daily_idx);
}

pub(super) fn draw_config(frame: &mut Frame, app: &App, area: Rect) {
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
            let inner_width = area.width.saturating_sub(2) as usize;
            let name_width = if area.width < 52 { 12 } else { 18 };
            let task_width = inner_width.saturating_sub(2 + 5 + name_width);
            ListItem::new(Line::from(vec![
                Span::styled(if selected { "▶ " } else { "  " }, style),
                Span::styled(format!("{enabled:<5}"), Style::default().fg(enabled_color)),
                Span::styled(
                    pad_display_width(
                        &truncate_display_width(&summary.name, name_width),
                        name_width,
                    ),
                    style,
                ),
                Span::styled(
                    truncate_display_width(&summary.task_type, task_width),
                    Style::default().fg(MUTED),
                ),
            ]))
        })
        .collect();
    render_list(frame, area, " 配置管理 ", items, app.config_idx);
}

pub(super) fn draw_add_task(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_task_edit(frame: &mut Frame, app: &App, area: Rect) {
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
            field_item(
                selected,
                field.label,
                &app.field_display(field, &value),
                chunks[1].width,
            )
        })
        .collect();
    render_list(frame, chunks[1], " 字段 ", items, app.field_idx);
}

pub(super) fn draw_variant_list(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_variant_edit(frame: &mut Frame, app: &App, area: Rect) {
    let fields = variant_fields();
    let items = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == app.field_idx;
            let value = app
                .variant_field_value(field)
                .unwrap_or_else(|| field.default.clone());
            field_item(
                selected,
                field.label,
                &app.field_display(field, &value),
                area.width,
            )
        })
        .collect();
    render_list(frame, area, " 编辑变体 ", items, app.field_idx);
}
