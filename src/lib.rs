use panic_handler::initialize_panic_handler;
use tracing::{info, level_filters::LevelFilter, Level};
use tracing_appender::non_blocking::WorkerGuard;

mod panic_handler;
mod tui;

pub fn run() -> color_eyre::Result<()> {
    initialize_panic_handler()?;
    let _log_guard = initialize_logging(Level::TRACE)?;
    tracing::info!("meow");
    // None::<u8>.unwrap();
    let ports = serialport::available_ports().expect("No ports found!");
    for p in ports {
        println!("{p:#?}");
        info!("{p:?}");
    }

    Ok(())
}

pub fn initialize_logging(max_level: Level) -> color_eyre::Result<WorkerGuard> {
    use rolling_file::{BasicRollingFileAppender, RollingConditionBasic};
    // use tracing::info;
    use tracing_subscriber::{filter, prelude::*};
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
