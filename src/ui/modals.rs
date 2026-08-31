//! 输入、选择、详情和确认弹窗渲染。

use super::chrome::{
    import_kind_span, input_dialog_hint, input_dialog_placeholder, shortcut_help_title,
    shortcut_hints,
};
use super::text::*;
use super::*;

pub(super) fn draw_input_dialog(frame: &mut Frame, area: Rect, input: &InputDialog) {
    let is_batch_import = input.allows_import_kind_switch();
    let popup = centered_rect(76, if is_batch_import { 9 } else { 7 }, area);
    let mut lines = vec![Line::from("")];
    if is_batch_import {
        let kind = input.import_kind().unwrap_or(ImportKind::Set);
        lines.push(Line::from(vec![
            Span::styled("  添加类型  ", Style::default().fg(MUTED)),
            import_kind_span("作业集", kind == ImportKind::Set),
            Span::raw("  "),
            import_kind_span("单个作业", kind == ImportKind::Single),
        ]));
        lines.push(Line::from(Span::styled(
            match kind {
                ImportKind::Set => "  添加完整作业集",
                ImportKind::Single => "  向批量列表追加单个作业",
            },
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(""));
    }
    let mut input_line = vec![Span::styled(
        "  > ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if input.value.is_empty() {
        input_line.push(Span::styled("█", Style::default().fg(ACCENT)));
        input_line.push(Span::styled(
            input_dialog_placeholder(input),
            Style::default().fg(MUTED),
        ));
    } else {
        input_line.push(Span::styled(
            &input.value,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::UNDERLINED),
        ));
        input_line.push(Span::styled("█", Style::default().fg(ACCENT)));
    }
    lines.push(Line::from(input_line));
    lines.push(Line::from(Span::styled(
        input_dialog_hint(input),
        Style::default().fg(MUTED),
    )));

    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .title(format!(" {} ", input.title)),
        ),
        popup,
    );
}

pub(super) fn draw_select_dialog(frame: &mut Frame, area: Rect, select: &SelectDialog) {
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

pub(super) fn draw_copilot_detail(frame: &mut Frame, area: Rect, detail: &mut CopilotDetailDialog) {
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
    draw_vscrollbar(frame, inner, content_rows, detail.scroll);
}

pub(super) fn draw_shortcut_help(frame: &mut Frame, area: Rect, app: &mut App) {
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
    draw_vscrollbar(frame, inner, content_rows, app.shortcut_help_scroll);
}

pub(super) fn draw_confirm_dialog(frame: &mut Frame, area: Rect, message: &str) {
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
