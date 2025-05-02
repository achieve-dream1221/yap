use std::{
    net::TcpStream,
    sync::{mpsc, Mutex},
};

use app::{App, CrosstermEvent};
use color_eyre::eyre::Context;
use panic_handler::initialize_panic_handler;
use ratatui::crossterm::{
    self,
    event::{DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEventKind},
    terminal,
};

use serialport::{SerialPortInfo, SerialPortType};
use tracing::{debug, error, info, level_filters::LevelFilter, Level};
use tracing_appender::non_blocking::WorkerGuard;
use tui::buffer::line_ending_iter;

mod app;
mod buffer;
mod event_carousel;
mod history;
mod macros;
mod notifications;
mod panic_handler;
mod serial;
mod settings;
mod traits;
mod tui;

/// Wrapper runner so any fatal errors get properly logged.
pub fn run() -> color_eyre::Result<()> {
    initialize_panic_handler()?;
    let _log_guard = initialize_logging(Level::TRACE)?;

    // let meow1 = b"1111\r\n22\r\n3333\n";
    // for meow in line_ending_iter(meow1, "\r\n").unwrap() {
    //     info!("MEOW: {meow:?}");
    // }

    // use ansi_to_tui::IntoText;
    // // let bytes = b"\x1b[38;2;225;192;203mAAAAA\x1b[0m".to_owned().to_vec();
    // let bytes = [
    //     27, 91, 48, 59, 51, 54, 109, 91, 68, 93, 91, 97, 112, 105, 46, 99, 111, 110, 110, 101, 99,
    //     116, 105, 111, 110, 58, 56, 50, 55, 93, 58, 32, 72, 111, 109, 101, 32, 65, 115, 115, 105,
    //     115, 116, 97, 110, 116, 32, 50, 48, 50, 53, 46, 51, 46, 52, 32, 40, 49, 57, 50, 46, 49, 54,
    //     56, 46, 56, 54, 46, 54, 41, 58, 32, 67, 111, 110, 110, 101, 99, 116, 101, 100, 32, 115,
    //     117, 99, 99, 101, 115, 115, 102, 117, 108, 108, 121, 27, 91, 48, 109, 13,
    // ];
    // let text = bytes.into_text().unwrap();
    // debug!("{text:#?}");

    let result = run_inner();
    if let Err(e) = &result {
        error!("Fatal error: {e}");
    }

    ratatui::restore();
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    result
    // std::thread::sleep(std::time::Duration::from_millis(500));
    // Ok(())
}

fn run_inner() -> color_eyre::Result<()> {
    // Err(color_eyre::Report::msg("AAA"))?;
    // None::<u8>.unwrap();

    let (tx, rx) = mpsc::channel::<app::Event>();
    let crossterm_tx = tx.clone();
    let crossterm_events = std::thread::spawn(move || -> color_eyre::Result<()> {
        loop {
            use crossterm::event::Event;
            use crossterm::event::KeyEventKind;
            match crossterm::event::read().unwrap() {
                Event::Resize(_, _) => crossterm_tx.send(CrosstermEvent::Resize.into())?,
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    crossterm_tx.send(CrosstermEvent::KeyPress(key).into())?;
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        crossterm_tx.send(CrosstermEvent::MouseScroll { up: true }.into())?
                    }
                    MouseEventKind::ScrollDown => {
                        crossterm_tx.send(CrosstermEvent::MouseScroll { up: false }.into())?
                    }
                    MouseEventKind::Down(button) => match button {
                        MouseButton::Left | MouseButton::Middle => (),
                        MouseButton::Right => {
                            crossterm_tx.send(CrosstermEvent::RightClick.into())?
                        }
                    },
                    _ => (),
                },
                _ => (),
            };
        }
    });

    // for p in ports {
    //     println!("{p:#?}");
    //     info!("{p:?}");
    // }
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = App::new(tx, rx).run(terminal);

    result
}

pub fn initialize_logging(max_level: Level) -> color_eyre::Result<WorkerGuard> {
    use rolling_file::{BasicRollingFileAppender, RollingConditionBasic};
    // use tracing::info;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{
        fmt::time::ChronoLocal, layer::SubscriberExt, util::SubscriberInitExt,
    };
    let log_name = std::env::current_exe()?
        .with_extension("log")
        // .file_name()
        // .expect("Couldn't build log path!")
        .to_owned();
    // let console = console_subscriber::spawn();
    let file_appender = BasicRollingFileAppender::new(
        log_name,
        RollingConditionBasic::new().max_size(1024 * 1024 * 5),
        2,
    )
    .unwrap();
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let time_fmt = ChronoLocal::new("%Y-%m-%d %H:%M:%S%.6f".to_owned());
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        // .pretty()
        .with_file(false)
        .with_ansi(false)
        .with_target(true)
        .with_timer(time_fmt.clone())
        .with_line_number(true)
        .with_filter(LevelFilter::from_level(max_level));

    // let (fmt_layer, reload_handle) = tracing_subscriber::reload::Layer::new(fmt_layer);
    // Allow everything through but limit lnk to just info, since it spits out a bit too much when reading shortcuts
    // let env_filter = tracing_subscriber::EnvFilter::new("trace,lnk=info");
    let registry = tracing_subscriber::registry()
        // .with(console)
        // .with(env_filter)
        .with(fmt_layer);

    // Try to connect to tcp_log_listener
    if let Ok(stream) = TcpStream::connect("127.0.0.1:7331") {
        let tcp_layer = tracing_subscriber::fmt::layer()
            .with_writer(Mutex::new(stream))
            // .pretty()
            .with_file(false)
            .with_ansi(true)
            .with_target(true)
            .with_timer(time_fmt)
            .with_line_number(true)
            .with_filter(LevelFilter::from_level(max_level));
        registry.with(tcp_layer).init();
    } else {
        registry.init();
    }
    Ok(guard)
}
