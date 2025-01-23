use app::App;
use panic_handler::initialize_panic_handler;
use tracing::{error, info, level_filters::LevelFilter, Level};
use tracing_appender::non_blocking::WorkerGuard;

mod app;
mod panic_handler;
mod settings;
mod tui;

/// Wrapper runner so any fatal errors get properly logged.
pub fn run() -> color_eyre::Result<()> {
    initialize_panic_handler()?;
    let _log_guard = initialize_logging(Level::TRACE)?;
    if let Err(e) = run_inner() {
        error!("Fatal error: {e}");
    }
    ratatui::restore();
    Ok(())
}

fn run_inner() -> color_eyre::Result<()> {
    // Err(color_eyre::Report::msg("AAA"))?;
    tracing::info!("meow");
    // None::<u8>.unwrap();
    let ports = serialport::available_ports().expect("No ports found!");
    for p in ports {
        println!("{p:#?}");
        info!("{p:?}");
    }
    let terminal = ratatui::init();
    let result = App::new().run(terminal);
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
        .with_timer(time_fmt)
        .with_line_number(true)
        .with_filter(LevelFilter::from_level(max_level));
    // let (fmt_layer, reload_handle) = tracing_subscriber::reload::Layer::new(fmt_layer);
    // Allow everything through but limit lnk to just info, since it spits out a bit too much when reading shortcuts
    // let env_filter = tracing_subscriber::EnvFilter::new("trace,lnk=info");
    tracing_subscriber::registry()
        // .with(console)
        // .with(env_filter)
        .with(fmt_layer)
        .init();
    Ok(guard)
}
