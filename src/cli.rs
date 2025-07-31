use camino::Utf8PathBuf;

#[derive(Debug, clap::Parser)]
/// For when you just need to quickly yap at a device
#[command(version, about)]
pub struct YapCli {
    /// Skip port selection, use given serial port path, or search for USB VID:PID[:SERIAL], exits if connection fails
    pub port: Option<String>,

    /// Override saved baud when connecting to [PORT]
    pub baud: Option<u32>,

    #[cfg(feature = "defmt")]
    /// Supply an ELF with defmt information to decode incoming serial data
    #[clap(short, long)]
    pub defmt_elf: Option<Utf8PathBuf>,

    /// Override path for configs, logs, macros, etc
    #[clap(short, long)]
    pub config_path: Option<Utf8PathBuf>,

    /// Print all built-in Actions to be used in keybinds
    #[clap(short, long)]
    pub print_actions: bool,
}

// #[derive(Debug, thiserror::Error)]
// pub enum CliError {
// }
