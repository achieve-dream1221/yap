#![deny(unused_must_use)]

use std::{
    net::{SocketAddr, TcpStream},
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use app::{App, CrosstermEvent};
use camino::Utf8PathBuf;

use clap::Parser;
use fs_err as fs;
use panic_handler::initialize_panic_handler;
use ratatui::crossterm::{
    self,
    event::{DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEventKind},
    terminal,
};

use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};
use tracing::{Level, debug, error, info, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;

use crate::cli::{CliError, YapCli};

mod app;
mod buffer;
mod cli;
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

static CONFIG_PARENT_PATH_CELL: OnceLock<Utf8PathBuf> = OnceLock::new();
pub fn config_adjacent_path<P: Into<Utf8PathBuf>>(path: P) -> Utf8PathBuf {
    let path = path.into();
    let config_path = CONFIG_PARENT_PATH_CELL.get_or_init(|| {
        determine_working_directory()
            .expect("failed to determine working directory")
            .try_into()
            .expect("working directory is not valid utf-8")
    });

    config_path.join(path)
}
static EXECUTABLE_FILE_STEM: OnceLock<Utf8PathBuf> = OnceLock::new();
pub fn get_executable_name() -> Utf8PathBuf {
    let exec_name = EXECUTABLE_FILE_STEM.get_or_init(|| {
        let exe_pathbuf = std::env::current_exe().expect("failed to get path of executable");
        let original = exe_pathbuf.with_extension("toml");
        let exec_name_str = original
            .file_stem()
            .expect("can't have file without name")
            .to_str()
            .expect("executable name is not valid utf-8");

        exec_name_str.into()
    });

    exec_name.to_owned()
}

/// Wrapper runner so any fatal errors get properly logged, and to
/// have a clear line between spinning up the whole app and all it's threads,
/// and just parsing CLI args and possibly exiting early.
pub fn run() -> color_eyre::Result<()> {
    let cli_args = YapCli::parse();

    if cli_args.print_actions {
        keybinds::print_all_actions();
        return Ok(());
    }

    // println!("{cli_args:#?}");
    // return Ok(());

    initialize_panic_handler()?;

    if let Some(path) = &cli_args.config_path {
        CONFIG_PARENT_PATH_CELL
            .set(path.to_owned())
            .expect("expected uninitialized cell");
    }

    let root_path = config_adjacent_path("");
    if !root_path.exists() {
        fs::create_dir_all(root_path)?;
    }

    let listener_address: SocketAddr = "127.0.0.1:7331".parse().unwrap();

    let mut log_path = config_adjacent_path(get_executable_name());
    log_path.set_extension("log");
    println!("{log_path}");
    let _log_guard = initialize_logging(Level::TRACE, log_path, Some(listener_address))?;

    let result = run_inner(cli_args);
    if let Err(e) = &result {
        error!("Fatal error: {e}");
    }

    result
}

fn run_inner(cli_args: YapCli) -> color_eyre::Result<()> {
    let (tx, rx) = crossbeam::channel::unbounded::<app::Event>();
    let crossterm_tx = tx.clone();
    let crossterm_thread = std::thread::spawn(move || {
        use crossterm::event::{Event, KeyEventKind};

        #[derive(Debug)]
        struct SendError;

        impl std::fmt::Display for SendError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "failed to send crossterm event")
            }
        }

        impl std::error::Error for SendError {}

        let filter_and_send = |event: Event| -> Result<(), SendError> {
            let send_event = |crossterm_event: CrosstermEvent| -> Result<(), SendError> {
                crossterm_tx
                    .send(crossterm_event.into())
                    .map_err(|_| SendError)?;
                Ok(())
            };
            match event {
                Event::Resize(_, _) => {
                    send_event(CrosstermEvent::Resize)?;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    send_event(CrosstermEvent::KeyPress(key))?
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        send_event(CrosstermEvent::MouseScroll { up: true })?;
                    }
                    MouseEventKind::ScrollDown => {
                        send_event(CrosstermEvent::MouseScroll { up: false })?;
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        send_event(CrosstermEvent::RightClick)?;
                    }
                    _ => (),
                },
                _ => (),
            }
            Ok(())
        };

        loop {
            let event = match crossterm::event::read() {
                Ok(ev) => ev,
                Err(e) => {
                    error!("error encountered when reading crossterm event, shutting down. {e}");
                    // return Err(e);
                    break;
                }
            };

            if let Err(e) = filter_and_send(event) {
                error!("{e}");
                break;
            }
        }
        debug!("Crossterm thread closed!");
    });

    // for p in ports {
    //     println!("{p:#?}");
    //     info!("{p:?}");
    // }

    // if let Err(e) = crokey::Combiner::default().enable_combining() {
    //     error!("Failed to enable key combining! {e}");
    // };

    let mut app = App::new(tx, rx);

    if let Some(defmt_path) = cli_args.defmt_elf {
        app.try_load_defmt_elf(&defmt_path)?;
    }

    if let Some(port) = cli_args.port {
        let mut usb_split = port.split(':');
        let first_part = usb_split.next();
        let second_part = usb_split.next();
        let third_part = usb_split.next();

        match (first_part, second_part, third_part) {
            // not a USB address
            (Some(_), None, None) => {
                let port_info = SerialPortInfo {
                    port_name: port,
                    port_type: SerialPortType::Unknown,
                };
                app.try_cli_connect(port_info, cli_args.baud)?;
                let terminal = ratatui::init();
                crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
                let result = app.run(terminal);
                ratatui::restore();
                crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
                result
            }
            // assume USB VID:PID[:SERIAL] format
            (Some(vid_str), Some(pid_str), serial) => {
                let port_info = SerialPortInfo {
                    port_name: String::new(),
                    port_type: SerialPortType::UsbPort(UsbPortInfo {
                        vid: u16::from_str_radix(vid_str, 16).map_err(CliError::VidParse)?,
                        pid: u16::from_str_radix(pid_str, 16).map_err(CliError::PidParse)?,
                        serial_number: serial.map(ToOwned::to_owned),

                        manufacturer: None,
                        product: None,
                    }),
                };
                app.try_cli_connect(port_info, cli_args.baud)?;
                let terminal = ratatui::init();
                crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
                let result = app.run(terminal);
                ratatui::restore();
                crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
                result
            }
            _ => {
                return Err(color_eyre::eyre::eyre!("Invalid USB address format"));
            }
        }
    } else {
        let terminal = ratatui::init();
        crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
        let result = app.run(terminal);
        ratatui::restore();
        crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
        result
    }
}

pub fn initialize_logging<P: AsRef<Path>>(
    max_level: Level,
    log_file_path: P,
    log_socket_addr: Option<SocketAddr>,
) -> color_eyre::Result<WorkerGuard> {
    let log_file_path = log_file_path.as_ref();
    use rolling_file::{BasicRollingFileAppender, RollingConditionBasic};
    // use tracing::info;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{
        fmt::time::ChronoLocal, layer::SubscriberExt, util::SubscriberInitExt,
    };
    // let console = console_subscriber::spawn();
    let file_appender = BasicRollingFileAppender::new(
        log_file_path,
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
    // Allow everything through but limit a few crate's levels.
    let env_filter = tracing_subscriber::EnvFilter::new("trace,espflash=info,notify=debug");
    let registry = tracing_subscriber::registry()
        // .with(console)
        .with(env_filter)
        .with(fmt_layer);

    // Try to connect to tcp_log_listener
    if let Some(listener_addr) = log_socket_addr
        && let Ok(stream) = TcpStream::connect_timeout(&listener_addr, Duration::from_millis(50))
    {
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
