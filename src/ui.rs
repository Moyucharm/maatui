//! TUI 渲染：分层菜单、配置编辑、自动战斗、日志与弹窗。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    App, CopilotDetailDialog, CopilotSection, EditorSection, InputDialog, LogScope, MainMenuItem,
    Screen, SelectDialog, TASK_TYPES, TaskPhase, variant_fields,
};
use crate::copilot::ImportKind;
use crate::runner::LogLevel;
use crate::shortcuts::{ShortcutHint, hints as shortcut_hints_for};

mod chrome;
mod copilot;
mod daily;
mod modals;
mod roguelike;
mod text;
mod update;

use chrome::{draw_footer, draw_logs, draw_running_control};
use copilot::draw_copilot;
use daily::{
    draw_add_task, draw_config, draw_daily, draw_task_edit, draw_variant_edit, draw_variant_list,
};
use modals::{
    draw_confirm_dialog, draw_copilot_detail, draw_input_dialog, draw_select_dialog,
    draw_shortcut_help,
};
use roguelike::draw_roguelike;
use text::{form_content_height, pad_display_width, render_list, truncate_display_width};
use update::draw_update;

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
        || matches!(
            app.screen,
            Screen::Daily | Screen::Copilot | Screen::Roguelike | Screen::Update
        );
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

/// 在内容区右缘叠加渲染垂直滚动条；内容未超出可视区时不渲染。
fn draw_vscrollbar(frame: &mut Frame, area: Rect, content_rows: u16, scroll: u16) {
    if content_rows <= area.height {
        return;
    }
    let scroll_positions = content_rows.saturating_sub(area.height).saturating_add(1);
    let mut state = ScrollbarState::new(scroll_positions as usize).position(scroll as usize);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(MUTED))
            .thumb_style(Style::default().fg(ACCENT)),
        area,
        &mut state,
    );
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
        Screen::Roguelike => draw_roguelike(frame, app, area),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CopilotTextField, ImportDestination, InputDialog, InputTarget};
    use chrome::{draw_logs, footer_keys, input_dialog_hint, shortcut_hints};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use text::log_style;

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
    fn vertical_scrollbar_reaches_track_bottom_at_max_scroll() {
        let backend = TestBackend::new(1, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_vscrollbar(frame, frame.area(), 20, 10))
            .unwrap();

        assert_eq!(terminal.backend().buffer().cell((0, 8)).unwrap().fg, ACCENT);
    }

    #[test]
    fn scrolling_logs_to_end_uses_wrapped_display_rows() {
        let mut app = App::new();
        app.screen = Screen::Daily;
        app.push_log_to(LogScope::Daily, LogLevel::Info, "x".repeat(38));
        app.push_log_to(LogScope::Daily, LogLevel::Info, "tail");
        let buffer = app.log_buffer_mut(LogScope::Daily);
        buffer.auto_scroll = false;
        buffer.scroll = u16::MAX;
        let backend = TestBackend::new(12, 6);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_logs(frame, &mut app, frame.area()))
            .unwrap();

        let buffer = app.log_buffer(LogScope::Daily);
        assert_eq!(buffer.scroll, 2);
        assert!(buffer.auto_scroll);
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("tail"), "rendered={rendered:?}");
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
            title: "添加到作业集".to_string(),
            value: String::new(),
            target: InputTarget::CopilotAdd {
                kind: ImportKind::Set,
                destination: ImportDestination::Batch,
            },
        };

        assert!(!input_dialog_hint(&normal).contains("← 作业集"));
        assert!(input_dialog_hint(&batch).contains("← 作业集"));
        assert!(input_dialog_hint(&batch).contains("→ 单个作业"));
    }

    #[test]
    fn batch_import_dialog_renders_clear_selected_mode_and_placeholder() {
        let mut input = InputDialog {
            title: "添加到作业集".to_string(),
            value: String::new(),
            target: InputTarget::CopilotAdd {
                kind: ImportKind::Set,
                destination: ImportDestination::Batch,
            },
        };
        let backend = TestBackend::new(100, 15);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_input_dialog(frame, frame.area(), &input))
            .unwrap();
        let set_text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .filter(|symbol| !symbol.trim().is_empty())
            .collect::<String>();
        assert!(set_text.contains("●作业集"));
        assert!(set_text.contains("○单个作业"));
        assert!(set_text.contains("添加完整作业集"));
        assert!(!set_text.contains("当前："));
        assert!(set_text.contains(">█prts://s12345/12345"));

        input.select_batch_import_kind(ImportKind::Single);
        terminal
            .draw(|frame| draw_input_dialog(frame, frame.area(), &input))
            .unwrap();
        let single_text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .filter(|symbol| !symbol.trim().is_empty())
            .collect::<String>();
        assert!(single_text.contains("○作业集"));
        assert!(single_text.contains("●单个作业"));
        assert!(single_text.contains("向批量列表追加"));
        assert!(!single_text.contains("当前："));
        assert!(single_text.contains(">█prts://12345/12345"));
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
    fn roguelike_layout_renders_control_advanced_tabs_and_logs() {
        let mut app = App::new();
        app.screen = Screen::Roguelike;
        app.push_log(LogLevel::Info, "自动肉鸽测试日志");
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let control = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .filter(|symbol| !symbol.trim().is_empty())
            .collect::<String>();
        assert!(control.contains("自动肉鸽"));
        assert!(control.contains("开始自动肉鸽"));
        assert!(control.contains("肉鸽主题"));
        assert!(control.contains("自动肉鸽日志"));

        app.roguelike_section_idx = 1;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let advanced = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .filter(|symbol| !symbol.trim().is_empty())
            .collect::<String>();
        assert!(advanced.contains("高级设置"));
        assert!(advanced.contains("探索次数上限"));
    }

    #[test]
    fn roguelike_layout_survives_narrow_terminals() {
        let mut app = App::new();
        app.screen = Screen::Roguelike;
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        assert_eq!(terminal.backend().buffer().area, Rect::new(0, 0, 40, 12));
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
