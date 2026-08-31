//! 更新页面渲染。

use super::text::*;
use super::*;

pub(super) fn draw_update(frame: &mut Frame, app: &App, area: Rect) {
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
