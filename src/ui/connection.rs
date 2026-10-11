//! 默认连接配置的紧凑字段列表。

use super::text::{field_item, panel};
use super::*;
use crate::connection::ConnectionField;

pub(super) fn draw_connection(frame: &mut Frame, app: &App, area: Rect) {
    let Some(config) = &app.connection_config else {
        let message = app.connection_error.as_deref().unwrap_or("连接配置未加载");
        frame.render_widget(
            Paragraph::new(format!("\n  × {message}\n\n  按 r 重试，Esc 返回"))
                .style(Style::default().fg(ERR))
                .wrap(Wrap { trim: false })
                .block(panel(" 连接配置 ")),
            area,
        );
        return;
    };

    let (list_area, error_area) = if app.connection_error.is_some() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)])
            .split(area);
        (chunks[0], Some(chunks[1]))
    } else {
        (area, None)
    };
    let selected = app.connection_idx.min(ConnectionField::ALL.len() - 1);
    let items = ConnectionField::ALL
        .iter()
        .enumerate()
        .map(|(index, &field)| {
            field_item(
                index == selected,
                field.label(),
                config.value(field).unwrap_or("默认 / 自动"),
                list_area.width,
            )
        })
        .collect();
    render_list(frame, list_area, " 连接配置 ", items, selected);

    if let (Some(error), Some(error_area)) = (&app.connection_error, error_area) {
        frame.render_widget(
            Paragraph::new(format!("× {error}"))
                .style(Style::default().fg(ERR))
                .wrap(Wrap { trim: false }),
            error_area,
        );
    }
}
