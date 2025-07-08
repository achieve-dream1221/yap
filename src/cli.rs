// desired interface:

// yap
//   - opens port selection menu

// yap COM4
//   - connects to given port at default baud (might need to be in config? 115200 if not set)
// yap COM4 9600
//   - connects to given port at given baud
// - if these fail, then show an error and sys::exit(1)
// - or instead, show an error and drop on port selection screen, but an --option can skip that and just close?
// - both should skip scanning for serial ports and try to directly connect to the given port
// - maybe also allow USB PID+VID as an "address"? would prefer to accept same formats as usb ignore configs

// option to print out all possible AppActions then exit?
// option for is_portable behavior

use std::num::ParseIntError;

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

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("error parsing USB VID")]
    VidParse(#[source] ParseIntError),
    #[error("error parsing USB PID")]
    PidParse(#[source] ParseIntError),
}
