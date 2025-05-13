use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use derivative::Derivative;
use fs_err::{self as fs};
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use serialport::{DataBits, FlowControl, Parity, StopBits};
use struct_table::StructTable;
use tracing::{info, level_filters::LevelFilter};

// Copied a lot from my other project, redefaulter
// https://github.com/nullstalgia/redefaulter/blob/ad81fad9468891b50daaac3215b0532386b6d1aa/src/settings/mod.rs

// TODO Cleaner defaults.
// What I have now works and is predictable,
// but there's a lot of gross repetition.
// Especially with needing both:
// - #[serde_inline_default] for when a _field_ is missing,
//   - Since #[serde(default)] gets the default for the field's _type_, and *not* the parent struct's `Default::default()` value for it
// - #[derivative(Default)] for properly setting up `Default::default()` for when a _struct_ is missing.

use crate::{app::DEFAULT_BAUD, serial::Reconnections, tui::buffer::UserEcho};

pub mod ser;
use ser::*;

pub mod line_ending;
use line_ending::*;

#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct Settings {
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(default)]
    pub rendering: Rendering,
    #[serde(default)]
    pub behavior: Behavior,
    #[serde(default)]
    pub misc: Misc,
    #[serde(default)]
    pub last_port_settings: PortSettings,
}

#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct Misc {
    #[serde_inline_default(String::from("debug"))]
    #[derivative(Default(value = "String::from(\"debug\")"))]
    pub log_level: String,
}

// TODO allow setting nicknames to devices?????

// TODO add Reset to Defaults somewhere in the UI

// TODO have flattened buffer behavior struct that gets sent to it on each change.

#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Rendering {
    #[serde_inline_default(UserEcho::All)]
    #[derivative(Default(value = "UserEcho::All"))]
    #[table(values = [UserEcho::None, UserEcho::All, UserEcho::NoBytes, UserEcho::NoMacros, UserEcho::NoMacrosOrBytes])]
    /// Show user input in buffer after sending.
    pub echo_user_input: UserEcho,

    #[serde(default)]
    /// Show timestamps next to each incoming line.
    pub timestamps: bool,

    #[serde(default)]
    /// Show buffer index and length next to line.
    pub show_indices: bool,

    #[serde(default)]
    /// Wrap text longer than the screen.
    pub wrap_text: bool,
}

#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Behavior {
    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Use text box to type in before sending, with history. If disabled, sends keyboard inputs directly (TODO).
    pub fake_shell: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Send symbols like \n or \xFF as their respective bytes.
    pub fake_shell_unescape: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Persist changes to Port Settings made while connected across sessions.
    pub retain_port_setting_changes: bool,

    #[serde(default)]
    /// Persist Fake Shell's command history across sessions (TODO).
    pub retain_history: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Fall back to macros with same name if category missing.
    pub fuzzy_macro_match: bool,

    #[serde_inline_default(Duration::from_millis(500))]
    #[derivative(Default(value = "Duration::from_millis(500)"))]
    #[table(allow_unknown_values)]
    #[table(display = Debug)]
    #[table(values = [Duration::from_millis(10), Duration::from_millis(100), Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1)])]
    #[serde(rename = "macro_chain_delay_ms")]
    #[serde(
        serialize_with = "serialize_duration_as_ms",
        deserialize_with = "deserialize_duration_from_ms"
    )]
    /// Pause between chained Macros.
    pub macro_chain_delay: Duration,
}

#[serde_inline_default]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, StructTable)]
pub struct PortSettings {
    /// The baud rate in symbols-per-second.
    // #[table(values = COMMON_BAUD)]
    #[table(immutable)]
    #[serde_inline_default(DEFAULT_BAUD)]
    pub baud_rate: u32,
    /// Number of bits per character.
    #[table(values = [DataBits::Five, DataBits::Six, DataBits::Seven, DataBits::Eight])]
    #[serde_inline_default(DataBits::Eight)]
    #[serde(
        serialize_with = "serialize_as_u8",
        deserialize_with = "deserialize_from_u8"
    )]
    pub data_bits: DataBits,
    /// Flow control modes.
    #[table(values = [FlowControl::None, FlowControl::Software, FlowControl::Hardware])]
    #[serde_inline_default(FlowControl::None)]
    pub flow_control: FlowControl,
    /// Parity bit modes.
    #[table(values = [Parity::None, Parity::Odd, Parity::Even])]
    #[serde_inline_default(Parity::None)]
    pub parity_bits: Parity,
    /// Number of stop bits.
    #[table(values = [StopBits::One, StopBits::Two])]
    #[serde_inline_default(StopBits::One)]
    #[serde(
        serialize_with = "serialize_as_u8",
        deserialize_with = "deserialize_from_u8"
    )]
    pub stop_bits: StopBits,
    /// Assert DTR to this state on port connect (and reconnect).
    #[table(rename = "DTR on Connect")]
    #[serde_inline_default(true)]
    pub dtr_on_connect: bool,
    /// Enable reconnections. Strict checks USB PID+VID+Serial#. Loose checks for any similar USB device/COM port.
    #[table(values = [Reconnections::Disabled, Reconnections::StrictChecks, Reconnections::LooseChecks])]
    #[serde_inline_default(Reconnections::LooseChecks)]
    pub reconnections: Reconnections,

    /// Line endings for RX'd data.
    #[table(display = ["\\n", "\\r", "\\r\\n", "None"])]
    #[table(rename = "RX Line Ending")]
    #[table(values = [RxLineEnding::Preset("\\n", &[b'\n']), RxLineEnding::Preset("\\r", &[b'\r']), RxLineEnding::Preset("\\r\\n", &[b'\r', b'\n']), RxLineEnding::Preset("", &[])])]
    #[table(allow_unknown_values)]
    #[serde(
        serialize_with = "serialize_as_string",
        deserialize_with = "deserialize_from_str"
    )]
    #[serde_inline_default(RxLineEnding::Preset("\\n", &[b'\n']))]
    pub rx_line_ending: RxLineEnding,
    /// Line endings for TX'd data.
    #[table(display = ["Inherit RX", "\\n", "\\r", "\\r\\n", "None"])]
    #[table(rename = "TX Line Ending")]
    #[table(values = [TxLineEnding::InheritRx, TxLineEnding::Preset("\\n", &[b'\n']), TxLineEnding::Preset("\\r", &[b'\r']), TxLineEnding::Preset("\\r\\n", &[b'\r', b'\n']), TxLineEnding::Preset("", &[])])]
    #[table(allow_unknown_values)]
    #[serde(
        serialize_with = "serialize_as_string",
        deserialize_with = "deserialize_from_str"
    )]
    #[serde_inline_default(TxLineEnding::InheritRx)]
    pub tx_line_ending: TxLineEnding,
    /// Default line ending for sent macros.
    #[table(display = ["Inherit TX", "Inherit RX", "\\n", "\\r", "\\r\\n", "None"])]
    #[table(values = [MacroTxLineEnding::InheritTx, MacroTxLineEnding::InheritRx, MacroTxLineEnding::Preset("\\n", &[b'\n']), MacroTxLineEnding::Preset("\\r", &[b'\r']), MacroTxLineEnding::Preset("\\r\\n", &[b'\r', b'\n']), MacroTxLineEnding::Preset("", &[])])]
    #[table(allow_unknown_values)]
    #[serde(
        serialize_with = "serialize_as_string",
        deserialize_with = "deserialize_from_str"
    )]
    #[serde_inline_default(MacroTxLineEnding::InheritTx)]
    pub macro_line_ending: MacroTxLineEnding,
}

impl Default for PortSettings {
    fn default() -> Self {
        Self {
            baud_rate: DEFAULT_BAUD,
            data_bits: DataBits::Eight,
            flow_control: FlowControl::None,
            parity_bits: Parity::None,
            stop_bits: StopBits::One,
            rx_line_ending: "\n".into(),
            tx_line_ending: TxLineEnding::InheritRx,
            macro_line_ending: MacroTxLineEnding::InheritTx,
            dtr_on_connect: true,
            reconnections: Reconnections::LooseChecks,
        }
    }
}

impl Settings {
    pub fn load(path: &Path, required: bool) -> color_eyre::Result<Self> {
        if !path.exists() && !required {
            let mut default = Settings::default();
            default.path = path.into();
            default.save()?;
            return Ok(default);
        } else if !path.exists() && required {
            // return Err(RedefaulterError::RequiredSettingsMissing);
            panic!("RequiredSettingsMissing");
        }
        let mut file = fs::File::open(path)?;
        let mut buffer = String::new();
        file.read_to_string(&mut buffer)?;
        drop(file);
        let mut config: Settings = toml::from_str(&buffer)?;
        config.path = path.into();
        config.save()?;
        Ok(config)
    }
    pub fn save(&self) -> color_eyre::Result<()> {
        assert_ne!(self.path.components().count(), 0);
        self.save_at(&self.path)?;
        Ok(())
    }
    fn save_at(&self, config_path: &Path) -> color_eyre::Result<()> {
        let toml_config = toml::to_string(self)?;
        info!("Serialized config length: {}", toml_config.len());
        let mut file = fs::File::create(config_path)?;
        file.write_all(toml_config.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    }
    pub fn get_log_level(&self) -> LevelFilter {
        LevelFilter::from_str(&self.misc.log_level).unwrap_or(LevelFilter::DEBUG)
    }
}
