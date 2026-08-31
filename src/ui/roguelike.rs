//! 自动肉鸽控制面板和高级设置渲染。

use super::text::*;
use super::*;
use crate::roguelike::RoguelikeSection;

pub(super) fn draw_roguelike(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    let tabs = Line::from(
        RoguelikeSection::ALL
            .iter()
            .enumerate()
            .flat_map(|(index, section)| {
                let selected = index == app.roguelike_section_idx;
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
    frame.render_widget(Paragraph::new(tabs).block(panel(" 自动肉鸽 ")), chunks[0]);

    if let Some(error) = app.roguelike_error.as_deref() {
        frame.render_widget(
            Paragraph::new(format!(
                "\n  × 自动肉鸽配置未加载\n\n  {error}\n\n  请修复配置文件后重新启动 MaaTUI。"
            ))
            .style(Style::default().fg(ERR))
            .wrap(Wrap { trim: false })
            .block(panel(" 自动肉鸽配置错误 ")),
            chunks[1],
        );
        return;
    }

    match app.roguelike_section() {
        RoguelikeSection::Control => draw_control(frame, app, chunks[1]),
        RoguelikeSection::Advanced => draw_advanced(frame, app, chunks[1]),
    }
}

fn draw_control(frame: &mut Frame, app: &App, area: Rect) {
    let fields = app.roguelike_visible_fields();
    let mut items = vec![field_item(
        app.roguelike_idx == 0,
        "开始自动肉鸽",
        "maa run <temporary-task> --batch -v",
    )];
    items.extend(fields.iter().enumerate().map(|(index, field)| {
        field_item(
            app.roguelike_idx == index + 1,
            field.label(),
            &app.roguelike_field_display(*field),
        )
    }));
    render_list(frame, area, " 控制面板 ", items, app.roguelike_idx);
}

fn draw_advanced(frame: &mut Frame, app: &App, area: Rect) {
    let fields = app.roguelike_visible_fields();
    if fields.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  当前主题与策略没有额外高级参数。")
                .style(Style::default().fg(MUTED))
                .block(panel(" 高级设置 ")),
            area,
        );
        return;
    }
    let items = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            field_item(
                app.roguelike_advanced_idx == index,
                field.label(),
                &app.roguelike_field_display(*field),
            )
        })
        .collect();
    render_list(frame, area, " 高级设置 ", items, app.roguelike_advanced_idx);
}
