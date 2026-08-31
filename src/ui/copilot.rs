//! Copilot 页面渲染。

use super::text::*;
use super::*;

pub(super) fn draw_copilot(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_single_copilot(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_copilot_list(frame: &mut Frame, app: &App, area: Rect) {
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

pub(super) fn draw_copilot_settings(frame: &mut Frame, app: &App, area: Rect) {
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
        ("清理干员头像缓存", "Enter 执行".to_string()),
    ];
    let items = rows
        .iter()
        .enumerate()
        .map(|(index, (label, value))| field_item(index == app.copilot_settings_idx, label, value))
        .collect();
    render_list(frame, area, " 运行设置 ", items, app.copilot_settings_idx);
}
