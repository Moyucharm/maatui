//! Header、运行控制、日志和 footer 渲染。

use std::collections::VecDeque;

use super::text::*;
use super::*;

pub(super) fn draw_running_control(frame: &mut Frame, app: &App, area: Rect) {
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
    let stop = "[ Esc / Enter / s ] 停止任务";
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

pub(super) fn draw_logs(frame: &mut Frame, app: &mut App, area: Rect) {
    let scope = app.visible_log_scope();
    let buffer = app.log_buffer(scope);
    let title = match scope {
        LogScope::Daily => " 每日任务日志 ",
        LogScope::Copilot => " 自动战斗日志 ",
        LogScope::Roguelike => " 自动肉鸽日志 ",
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
    // 使用与 Paragraph::wrap(trim = false) 相同的断词规则计算真实滚动范围。
    let content_width = inner.width.max(1);
    let content_rows = lines.iter().fold(0_u16, |rows, line| {
        rows.saturating_add(wrapped_line_count(line, content_width))
    });
    let max_scroll = content_rows.saturating_sub(inner.height);
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    if buffer.auto_scroll {
        buffer.scroll = max_scroll;
    } else {
        buffer.scroll = buffer.scroll.min(max_scroll);
        if buffer.scroll >= max_scroll {
            buffer.auto_scroll = true;
        }
    }
    frame.render_widget(paragraph.scroll((buffer.scroll, 0)), inner);
    draw_vscrollbar(frame, inner, content_rows, buffer.scroll);
}

fn wrapped_line_count(line: &Line<'_>, max_width: u16) -> u16 {
    let mut rows = 0_u16;
    let mut line_width = 0_u16;
    let mut word_width = 0_u16;
    let mut whitespace_width = 0_u16;
    let mut pending_line_empty = true;
    let mut pending_word_empty = true;
    let mut pending_whitespace = VecDeque::new();
    let mut previous_was_non_whitespace = false;

    for grapheme in line.styled_graphemes(Style::default()) {
        let is_whitespace = grapheme.is_whitespace();
        let symbol_width = grapheme.symbol.width() as u16;
        if symbol_width > max_width {
            continue;
        }

        let word_found = previous_was_non_whitespace && is_whitespace;
        let pending_segment_overflows = pending_line_empty
            && word_width
                .saturating_add(whitespace_width)
                .saturating_add(symbol_width)
                > max_width;
        if word_found || pending_segment_overflows {
            line_width = line_width
                .saturating_add(whitespace_width)
                .saturating_add(word_width);
            pending_line_empty = false;
            pending_whitespace.clear();
            whitespace_width = 0;
            word_width = 0;
            pending_word_empty = true;
        }

        let line_full = line_width >= max_width;
        let pending_word_overflows = symbol_width > 0
            && line_width
                .saturating_add(whitespace_width)
                .saturating_add(word_width)
                >= max_width;
        if line_full || pending_word_overflows {
            rows = rows.saturating_add(1);
            let mut remaining_width = max_width.saturating_sub(line_width);
            line_width = 0;
            pending_line_empty = true;

            while pending_whitespace
                .front()
                .is_some_and(|width| *width <= remaining_width)
            {
                let width = pending_whitespace.pop_front().unwrap_or_default();
                whitespace_width = whitespace_width.saturating_sub(width);
                remaining_width = remaining_width.saturating_sub(width);
            }
            if is_whitespace && pending_whitespace.is_empty() {
                previous_was_non_whitespace = false;
                continue;
            }
        }

        if is_whitespace {
            whitespace_width = whitespace_width.saturating_add(symbol_width);
            pending_whitespace.push_back(symbol_width);
        } else {
            word_width = word_width.saturating_add(symbol_width);
            pending_word_empty = false;
        }
        previous_was_non_whitespace = !is_whitespace;
    }

    if !pending_line_empty || !pending_word_empty || !pending_whitespace.is_empty() {
        rows.saturating_add(1)
    } else {
        rows.max(1)
    }
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let available = area.width.saturating_sub(1) as usize;
    let keys = footer_keys(app, available);
    frame.render_widget(
        Paragraph::new(Span::styled(format!(" {keys}"), Style::default().fg(MUTED))),
        area,
    );
}

pub(super) fn footer_keys(app: &App, width: usize) -> String {
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

pub(super) fn modal_footer_keys(app: &App) -> Option<&'static str> {
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

pub(super) fn shortcut_hints(app: &App) -> Vec<ShortcutHint> {
    shortcut_hints_for(app.shortcut_context())
}

pub(super) fn shortcut_help_title(app: &App) -> String {
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
        Screen::Roguelike => format!("自动肉鸽 · {}", app.roguelike_section().label()),
        Screen::Update => "更新管理".to_string(),
    }
}

pub(super) fn input_dialog_hint(input: &InputDialog) -> &'static str {
    if input.allows_import_kind_switch() {
        "  Enter 确认 · Esc 取消 · Ctrl+U 清空 · ← 作业集 · → 单个作业"
    } else {
        "  Enter 确认 · Esc 取消 · Ctrl+U 清空"
    }
}

pub(super) fn import_kind_span(label: &'static str, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(
            format!(" ● {label} "),
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(format!(" ○ {label} "), Style::default().fg(MUTED))
    }
}

pub(super) fn input_dialog_placeholder(input: &InputDialog) -> &'static str {
    match input.import_kind() {
        Some(ImportKind::Set) => "prts://s12345 / 12345",
        Some(ImportKind::Single) => "prts://12345 / 12345",
        None => "",
    }
}
