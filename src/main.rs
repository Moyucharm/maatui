//! MaaTUI — 本地 maa 的轻量 TUI 控制壳。

mod app;
mod runner;
mod ui;

use std::io::{self, stdout};
use std::panic;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, TaskPhase};

fn main() {
    if let Err(err) = run() {
        eprintln!("MaaTUI 错误: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    install_panic_hook();

    let mut terminal = setup_terminal()?;
    let mut app = App::new();

    let result = main_loop(&mut terminal, &mut app);

    app.cleanup();
    restore_terminal(&mut terminal)?;

    result
}

fn main_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    let tick = Duration::from_millis(100);
    let mut quit_since: Option<Instant> = None;

    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        app.handle_key(key);
                    }
                }
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        app.tick();

        if app.should_quit {
            if app.phase == TaskPhase::Idle {
                break;
            }
            let since = quit_since.get_or_insert_with(Instant::now);
            // 运行中按 q：先停任务，最多等 ~3s，随后 force_cleanup。
            if since.elapsed() >= Duration::from_secs(3) {
                break;
            }
        }
    }

    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(out);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let mut out = stdout();
        let _ = execute!(out, LeaveAlternateScreen, DisableMouseCapture);
        original(info);
    }));
}
