use color_eyre::Result;
use ratatui::crossterm::event::DisableMouseCapture;

pub fn initialize_panic_handler() -> Result<()> {
    let (panic_hook, eyre_hook) = color_eyre::config::HookBuilder::default()
        .panic_section(format!(
            "This is a bug. Consider reporting it at {}",
            env!("CARGO_PKG_REPOSITORY")
        ))
        .display_location_section(true)
        .display_env_section(true)
        .into_hooks();
    eyre_hook.install()?;
    std::panic::set_hook(Box::new(move |panic_info| {
        _ = ratatui::restore();
        _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);

        let msg = format!("{}", panic_hook.panic_report(panic_info));
        #[cfg(not(debug_assertions))]
        {
            eprintln!("{msg}");
            use human_panic::{handle_dump, print_msg, Metadata};
            let author = format!("authored by {}", env!("CARGO_PKG_AUTHORS"));
            let support = format!(
                "You can open a support request at {}",
                env!("CARGO_PKG_REPOSITORY")
            );
            let meta = Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                .authors(author)
                .support(support);

            let file_path = handle_dump(&meta, panic_info);
            print_msg(file_path, &meta)
                .expect("human-panic: printing error message to console failed");
        }
        tracing::log::error!("Error: {}", strip_ansi_escapes::strip_str(msg));

        #[cfg(debug_assertions)]
        {
            // Better Panic stacktrace that is only enabled when debugging.
            better_panic::Settings::auto()
                .most_recent_first(false)
                .lineno_suffix(true)
                .verbosity(better_panic::Verbosity::Full)
                .create_panic_handler()(panic_info);
        }

        std::process::exit(libc::EXIT_FAILURE);
    }));
    Ok(())
}
