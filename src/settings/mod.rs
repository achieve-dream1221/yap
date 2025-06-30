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
use strum::VariantArray;
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

use crate::{
    app::DEFAULT_BAUD,
    buffer::UserEcho,
    serial::{IgnoreableUsb, Reconnections},
};

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
    #[cfg(feature = "defmt")]
    #[serde(default)]
    pub defmt: Defmt,
    #[cfg(feature = "logging")]
    #[serde(default)]
    pub logging: Logging,
    #[serde(default)]
    pub ignored: Ignored,
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
macro_rules! inclusive_increment {
    ($len:expr) => {{
        const LEN: usize = $len + 1;
        let mut arr = [0; LEN];
        let mut i = 0;
        while i < LEN {
            arr[i] = i;
            i += 1;
        }
        arr
    }};
}
#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Rendering {
    #[serde_inline_default(UserEcho::All)]
    #[derivative(Default(value = "UserEcho::All"))]
    #[table(values = UserEcho::VARIANTS)]
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

    #[serde(default)]
    /// Show line ending at end of recieved lines.
    pub show_line_ending: bool,

    #[serde(default)]
    /// Show invalid byte sequences in \xFF notation.
    pub escape_invalid_bytes: bool,

    #[serde(default)]
    /// Show recieved bytes in a Hex+ASCII view.
    pub hex_view: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Show Address+Offset Markers+ASCII label above hex view.
    pub hex_view_header: bool,

    #[serde(default)]
    #[table(values = inclusive_increment!(48))]
    #[table(allow_unknown_values)]
    /// Set an optional maximum bytes per line.
    pub bytes_per_line: MaxBytesPerLine,

    #[serde_inline_default(HexHighlightStyle::HighlightAsciiSymbols)]
    #[derivative(Default(value = "HexHighlightStyle::HighlightAsciiSymbols"))]
    #[table(values = HexHighlightStyle::VARIANTS)]
    /// Show user input in buffer after sending.
    pub hex_view_highlights: HexHighlightStyle,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Serialize, Deserialize, strum::Display, strum::VariantArray,
)]
#[strum(serialize_all = "title_case")]
pub enum HexHighlightStyle {
    None,
    DarkenNulls,
    #[strum(serialize = "Highlight ASCII Symbols")]
    HighlightAsciiSymbols,
    // HighlightUnicode TODO
    // UseColorRules TODO
    // might need to do something wild like have a buffer with tagged-with-index spans
    StyleA,
    StyleB,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[repr(transparent)]
pub struct MaxBytesPerLine(u8);
impl PartialEq<u8> for MaxBytesPerLine {
    fn eq(&self, other: &u8) -> bool {
        &self.0 == other
    }
}

impl PartialEq<MaxBytesPerLine> for u8 {
    fn eq(&self, other: &MaxBytesPerLine) -> bool {
        self == &other.0
    }
}

impl PartialEq<usize> for MaxBytesPerLine {
    fn eq(&self, other: &usize) -> bool {
        usize::from(self.0) == *other
    }
}

impl PartialEq<MaxBytesPerLine> for usize {
    fn eq(&self, other: &MaxBytesPerLine) -> bool {
        *self == usize::from(other.0)
    }
}

impl From<u8> for MaxBytesPerLine {
    fn from(value: u8) -> Self {
        MaxBytesPerLine(value)
    }
}

impl From<usize> for MaxBytesPerLine {
    fn from(value: usize) -> Self {
        MaxBytesPerLine(value as u8)
    }
}

impl From<MaxBytesPerLine> for u8 {
    fn from(value: MaxBytesPerLine) -> Self {
        value.0
    }
}

impl From<MaxBytesPerLine> for usize {
    fn from(value: MaxBytesPerLine) -> Self {
        value.0 as usize
    }
}

impl From<MaxBytesPerLine> for Option<u8> {
    fn from(value: MaxBytesPerLine) -> Self {
        match value.0 {
            0 => None,
            x => Some(x),
        }
    }
}

impl std::fmt::Display for MaxBytesPerLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0 => write!(f, "Fit to Screen"),
            x => write!(f, "{x}"),
        }
    }
}

#[derive(
    Debug, Default, Clone, PartialEq, Serialize, Deserialize, strum::VariantArray, strum::Display,
)]
// #[strum(serialize_all = "title_case")]
pub enum LoggingType {
    #[default]
    #[strum(serialize = "Text Only")]
    Text,
    #[strum(serialize = "Binary Only")]
    Binary,
    #[strum(serialize = "Text + Binary")]
    Both,
}

#[cfg(feature = "logging")]
#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Logging {
    #[serde(default)]
    #[table(values = LoggingType::VARIANTS)]
    /// Whether to log to a text file, raw binary, or both.
    pub log_file_type: LoggingType,

    // pub path: PathBuf,
    /// Automatically begin logging on successful port connection.
    pub always_begin_on_connect: bool,

    #[serde_inline_default(String::from(crate::buffer::DEFAULT_TIMESTAMP_FORMAT))]
    #[derivative(Default(value = "String::from(crate::buffer::DEFAULT_TIMESTAMP_FORMAT)"))]
    #[table(skip)]
    /// Show timestamps next to each line in text outputs.
    pub timestamp: String,

    // Just always doing instead right now, no need for the option.
    // #[serde_inline_default(true)]
    // #[derivative(Default(value = "true"))]
    // /// Escape invalid UTF-8 byte sequences in \xFF notation in text outputs.
    // pub escape_unprintable_bytes: bool,
    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Record user input in text outputs.
    pub log_user_input: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// When enabled, active log files persist across devices.
    pub keep_log_across_devices: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Log any disconnect and reconnect events in text outputs.
    pub log_connection_events: bool,
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

    #[serde_inline_default(Duration::from_millis(500))]
    #[derivative(Default(value = "Duration::from_millis(500)"))]
    #[table(allow_unknown_values)]
    #[table(display = Debug)]
    #[table(values = [Duration::from_millis(10), Duration::from_millis(100), Duration::from_millis(250), Duration::from_millis(500), Duration::from_secs(1)])]
    #[serde(rename = "action_chain_delay_ms")]
    #[serde(
        serialize_with = "serialize_duration_as_ms",
        deserialize_with = "deserialize_duration_from_ms"
    )]
    /// Default delay between chained Actions in keybinds. Can be overwritten with "pause_ms:XXX" in chains.
    pub action_chain_delay: Duration,

    #[cfg(feature = "macros")]
    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Fall back to macros with same name if category missing.
    pub fuzzy_macro_match: bool,
}

#[cfg(feature = "defmt")]
#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    strum::VariantArray,
    strum::EnumString,
    strum::Display,
)]
pub enum DefmtSupport {
    FramedRzcobs,
    UnframedRzcobs,
    Raw,
    #[default]
    Disabled,
}

#[cfg(feature = "defmt")]
#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    strum::VariantArray,
    strum::EnumString,
    strum::Display,
    strum::EnumIs,
)]
pub enum DefmtLocation {
    #[default]
    Full,
    Shortened,
    Hidden,
}

#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    strum::VariantArray,
    strum::EnumString,
    strum::Display,
)]
#[strum(serialize_all = "title_case")]
#[strum(ascii_case_insensitive)]
pub enum Level {
    #[default]
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[cfg(feature = "defmt")]
impl From<Level> for defmt_parser::Level {
    fn from(value: Level) -> Self {
        match value {
            Level::Trace => defmt_parser::Level::Trace,
            Level::Debug => defmt_parser::Level::Debug,
            Level::Info => defmt_parser::Level::Info,
            Level::Warn => defmt_parser::Level::Warn,
            Level::Error => defmt_parser::Level::Error,
        }
    }
}
#[cfg(feature = "defmt")]
impl From<defmt_parser::Level> for Level {
    fn from(value: defmt_parser::Level) -> Self {
        match value {
            defmt_parser::Level::Trace => Level::Trace,
            defmt_parser::Level::Debug => Level::Debug,
            defmt_parser::Level::Info => Level::Info,
            defmt_parser::Level::Warn => Level::Warn,
            defmt_parser::Level::Error => Level::Error,
        }
    }
}

#[cfg(feature = "defmt")]
#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Defmt {
    #[serde(default)]
    #[table(values = DefmtSupport::VARIANTS)]
    /// Enable parsing RX'd serial data as defmt packets.
    pub defmt_parsing: DefmtSupport,

    #[cfg(feature = "defmt_watch")]
    #[table(rename = "Watch ELF for Changes")]
    /// Reload defmt data from ELF when file is updated.
    pub watch_elf_for_changes: bool,

    #[serde_inline_default(Level::Trace)]
    #[derivative(Default(value = "Level::Trace"))]
    #[table(display = Debug)]
    #[table(values = Level::VARIANTS)]
    /// Maximum log level to display. Items without a level are always shown.
    pub max_log_level: Level,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Show device-derived timestamps, if available.
    pub device_timestamp: bool,

    #[serde(default)]
    #[table(values = DefmtLocation::VARIANTS)]
    /// Show module where log originated from, if available.
    pub show_module: DefmtLocation,

    #[serde(default)]
    #[table(values = DefmtLocation::VARIANTS)]
    /// Show file where log originated from, if available.
    pub show_file: DefmtLocation,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Show line number in file where log originated from, if available.
    pub show_line_number: bool,
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
    #[table(values = Reconnections::VARIANTS)]
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

    #[cfg(feature = "macros")]
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

#[serde_inline_default]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Ignored {
    #[serde(default)]
    /// Show invalid byte sequences in \xFF notation.
    pub usb: Vec<IgnoreableUsb>,
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
            #[cfg(feature = "macros")]
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
