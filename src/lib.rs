#![deny(unused_must_use)]

use std::{
    net::{SocketAddr, TcpStream},
    sync::{Mutex, mpsc},
    time::Duration,
};

use app::{App, CrosstermEvent};
use color_eyre::eyre::Context;
use fs_err as fs;
use panic_handler::initialize_panic_handler;
use ratatui::crossterm::{
    self,
    event::{DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEventKind},
    terminal,
};

use serialport::{SerialPortInfo, SerialPortType};
use tracing::{Level, debug, error, info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;

mod app;
mod buffer;
mod errors;
mod event_carousel;
mod history;
mod keybinds;
#[cfg(feature = "macros")]
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
    let working_directory = determine_working_directory().unwrap();
    if !working_directory.exists() {
        fs::create_dir(&working_directory)?;
    }
    std::env::set_current_dir(&working_directory).expect("Failed to change working directory");
    let _log_guard = initialize_logging(Level::TRACE)?;

    // let meow1 = b"1111\r\n22\r\n3333\n";
    // for meow in line_ending_iter(meow1, "\r\n").unwrap() {
    //     info!("MEOW: {meow:?}");
    // }

    // use ansi_to_tui::IntoText;
    // // let bytes = b"\x1b[38;2;225;192;203mAAAAA\x1b[0m".to_owned().to_vec();
    // let bytes = [
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

    let (tx, rx) = crossbeam::channel::unbounded::<app::Event>();
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
    // if let Err(e) = crokey::Combiner::default().enable_combining() {
    //     error!("Failed to enable key combining! {e}");
    // };
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
        .file_name()
        .expect("Couldn't build log path!")
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
    let env_filter = tracing_subscriber::EnvFilter::new("trace,espflash=info");
    let registry = tracing_subscriber::registry()
        // .with(console)
        .with(env_filter)
        .with(fmt_layer);

    // Try to connect to tcp_log_listener
    let listener_address: SocketAddr = "127.0.0.1:7331".parse().unwrap();

    if let Ok(stream) = TcpStream::connect_timeout(&listener_address, Duration::from_millis(50)) {
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

/// Returns the directory that logs, config, and other files should be placed in by default.
// The rules for how it determines the directory is as follows:
// If the app is built with the portable feature, it will just return it's parent directory.
// If there is a config file present adjacent to the executable, the executable's parent path is returned.
// Otherwise, it will return the `directories` `config_dir` output.
//
// Debug builds are always portable. Release builds can optionally have the "portable" feature enabled.
fn determine_working_directory() -> Option<std::path::PathBuf> {
    let portable = is_portable();
    let exe_path = std::env::current_exe().expect("Failed to get executable path");
    let exe_parent = exe_path
        .parent()
        .expect("Couldn't get parent dir of executable")
        .to_path_buf();
    let config_path = exe_path.with_extension("toml");

    if portable || config_path.exists() {
        Some(exe_parent)
    } else {
        get_user_dir()
    }
}

#[cfg(any(debug_assertions, feature = "portable"))]
fn is_portable() -> bool {
    true
}

#[cfg(not(any(debug_assertions, feature = "portable")))]
fn is_portable() -> bool {
    false
}

#[cfg(any(debug_assertions, feature = "portable"))]
fn get_user_dir() -> Option<std::path::PathBuf> {
    None
}

#[cfg(not(any(debug_assertions, feature = "portable")))]
fn get_user_dir() -> Option<std::path::PathBuf> {
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let mut config_dir = base_dirs.config_dir().to_owned();
        config_dir.push(env!("CARGO_PKG_NAME"));
        Some(config_dir)
    } else {
        None
    }
}

#[macro_export]
/// Macro to check if any field has changed between two objects.
///
/// Usage: `changed!(old, new, field_name1, field_name2, ...)`
macro_rules! changed {
    // I technically don't need to keep the single field version,
    // but rust-analyzer doesn't suggest field names on the multi-field version,
    // and I wanted to keep the ergonomics of using LSP-suggested names
    // instead of going to the type myself.
    ($a:expr, $b:expr, $field:ident) => {
        ($a.$field != $b.$field)
    };
    ($a:expr, $b:expr, $($field:ident),+) => {
        {
            false $(|| $a.$field != $b.$field)+
        }
    };
}
