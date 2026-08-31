//! UI 文本与通用控件工具。

use super::*;

pub(super) fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    selected: usize,
) {
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(selected.min(items.len() - 1)));
    }
    let item_count = items.len() as u16;
    let inner = panel(title).inner(area);
    frame.render_stateful_widget(List::new(items).block(panel(title)), area, &mut state);
    // List 内部自动滚动，渲染后用实际 offset 画出位置指示。
    draw_vscrollbar(frame, inner, item_count, state.offset() as u16);
}

const FIELD_LABEL_WIDTH: usize = 20;

pub(super) fn field_item(selected: bool, label: &str, value: &str) -> ListItem<'static> {
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

pub(super) fn truncate_display_width(text: &str, max_width: usize) -> String {
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

pub(super) fn pad_display_width(text: &str, width: usize) -> String {
    let padding = width.saturating_sub(text.width());
    format!("{text}{}", " ".repeat(padding))
}

pub(super) fn panel<'a>(title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .title(Span::styled(title, Style::default().fg(ACCENT)))
}

/// Idle + 日志页的表单高度：内容行 + 上下边框，并给日志至少留 8 行。
pub(super) fn form_content_height(app: &App, total_height: u16) -> u16 {
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
        Screen::Roguelike => match app.roguelike_section() {
            crate::roguelike::RoguelikeSection::Control => app.roguelike_control_row_count() as u16,
            crate::roguelike::RoguelikeSection::Advanced => {
                app.roguelike_advanced_row_count().max(1) as u16
            }
        },
        Screen::Update => unreachable!("更新页已使用专用布局"),
    };
    let desired = rows.saturating_add(BORDER).max(FORM_MIN);
    let reserved = HEADER.saturating_add(FOOTER).saturating_add(LOGS_MIN);
    let available = total_height.saturating_sub(reserved).max(FORM_MIN);
    desired.min(available)
}

pub(super) fn log_style(level: LogLevel) -> (&'static str, Color, Color) {
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

pub(super) fn on_off(value: bool) -> String {
    if value { "开" } else { "关" }.to_string()
}

pub(super) fn copilot_support_label(value: i64) -> String {
    match value {
        0 => "不使用助战".to_string(),
        1 => "仅补一名缺失".to_string(),
        2 => "缺失时补齐，否则指定".to_string(),
        3 => "缺失时补齐，否则随机".to_string(),
        other => format!("未知模式 ({other})"),
    }
}

pub(super) fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
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
