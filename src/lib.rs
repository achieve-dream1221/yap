use std::{
    net::TcpStream,
    sync::{mpsc, Mutex},
};

use app::{App, CrosstermEvent};
use color_eyre::eyre::Context;
use panic_handler::initialize_panic_handler;
use ratatui::crossterm::{
    self,
    event::{DisableMouseCapture, EnableMouseCapture, MouseEventKind},
    terminal,
};
use serialport::{SerialPortInfo, SerialPortType};
use tracing::{error, info, level_filters::LevelFilter, Level};
use tracing_appender::non_blocking::WorkerGuard;

mod app;
mod panic_handler;
mod serial;
mod settings;
mod tui;

/// Wrapper runner so any fatal errors get properly logged.
pub fn run() -> color_eyre::Result<()> {
    initialize_panic_handler()?;
    let _log_guard = initialize_logging(Level::TRACE)?;
    let result = run_inner();
    if let Err(e) = &result {
        error!("Fatal error: {e}");
    }

    ratatui::restore();
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)?;
    result
}

fn run_inner() -> color_eyre::Result<()> {
    // Err(color_eyre::Report::msg("AAA"))?;
    // None::<u8>.unwrap();

    let (tx, rx) = mpsc::channel::<app::Event>();
    let crossterm_tx = tx.clone();
    let crossterm_events =
        std::thread::spawn(move || -> color_eyre::Result<()> {
            loop {
                use crossterm::event::Event;
                use crossterm::event::KeyEventKind;
                match crossterm::event::read().unwrap() {
                    Event::Resize(_, _) => {
                        crossterm_tx.send(app::Event::Crossterm(CrosstermEvent::Resize))?
                    }
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        crossterm_tx.send(app::Event::Crossterm(CrosstermEvent::KeyPress(key)))?;
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => crossterm_tx.send(app::Event::Crossterm(
                            CrosstermEvent::MouseScroll { up: true },
                        ))?,
                        MouseEventKind::ScrollDown => crossterm_tx.send(app::Event::Crossterm(
                            CrosstermEvent::MouseScroll { up: false },
                        ))?,
                        _ => (),
                    },
                    _ => (),
                };
            }
        });

    let mut ports = serialport::available_ports().wrap_err("No ports found!")?;
    ports.push(SerialPortInfo {
        port_name: "virtual-port".to_owned(),
        port_type: SerialPortType::Unknown,
    });
    // TODO: Add filters for this in UI
    #[cfg(unix)]
    let ports: Vec<_> = ports
        .into_iter()
        .filter(|port| {
            !(port.port_type == SerialPortType::Unknown && !port.port_name.starts_with("virtual"))
        })
        .collect();

    // let mut tui = tui::Tui::new(rx, ports);

    tracing::info!("meow");
    // for p in ports {
    //     println!("{p:#?}");
    //     info!("{p:?}");
    // }
    let terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)?;
    let result = App::new(tx, rx, ports).run(terminal);

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
