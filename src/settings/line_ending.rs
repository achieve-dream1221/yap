use bstr::{ByteSlice, ByteVec};
use compact_str::{CompactString, ToCompactString};
use std::str::FromStr;

pub const PRESET_LINE_ENDINGS: &[&'static str] = &["\n", "\r", "\r\n", ""];
pub const PRESET_LINE_ENDINGS_ESCAPED: &[&'static str] = &["\\n", "\\r", "\\r\\n", ""];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RxLineEnding {
    Preset(&'static str, &'static [u8]),
    Custom(CompactString, Vec<u8>),
}

impl RxLineEnding {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            RxLineEnding::Preset(_, preset) => preset,
            RxLineEnding::Custom(_, custom) => custom,
        }
    }
}

impl std::fmt::Display for RxLineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preset(preset, _) => write!(f, "{preset}"),
            Self::Custom(custom, _) => write!(f, "{custom}"),
        }
    }
}

impl FromStr for RxLineEnding {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            escaped if PRESET_LINE_ENDINGS_ESCAPED.contains(&escaped) => {
                let index = PRESET_LINE_ENDINGS_ESCAPED
                    .iter()
                    .position(|le| *le == escaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            unescaped if PRESET_LINE_ENDINGS.contains(&unescaped) => {
                let index = PRESET_LINE_ENDINGS
                    .iter()
                    .position(|le| *le == unescaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            other => {
                let into_bytes = Vec::unescape_bytes(other);
                Ok(Self::Custom(
                    into_bytes.escape_bytes().to_compact_string(),
                    into_bytes,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxLineEnding {
    InheritRx,
    Preset(&'static str, &'static [u8]),
    Custom(CompactString, Vec<u8>),
}

impl TxLineEnding {
    pub fn as_bytes<'a>(&'a self, rx: &'a RxLineEnding) -> &'a [u8] {
        match self {
            TxLineEnding::InheritRx => rx.as_bytes(),
            TxLineEnding::Preset(_, preset) => preset,
            TxLineEnding::Custom(_, custom) => custom,
        }
    }
}

impl std::fmt::Display for TxLineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InheritRx => write!(f, "InheritRX"),
            Self::Preset(preset, _) => write!(f, "{preset}"),
            Self::Custom(custom, _) => write!(f, "{custom}"),
        }
    }
}

impl FromStr for TxLineEnding {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            _ if s.eq_ignore_ascii_case("InheritRX") => Ok(Self::InheritRx),

            escaped if PRESET_LINE_ENDINGS_ESCAPED.contains(&escaped) => {
                let index = PRESET_LINE_ENDINGS_ESCAPED
                    .iter()
                    .position(|le| *le == escaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            unescaped if PRESET_LINE_ENDINGS.contains(&unescaped) => {
                let index = PRESET_LINE_ENDINGS
                    .iter()
                    .position(|le| *le == unescaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            other => {
                let into_bytes = Vec::unescape_bytes(other);
                Ok(Self::Custom(
                    into_bytes.escape_bytes().to_compact_string(),
                    into_bytes,
                ))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroTxLineEnding {
    InheritRx,
    InheritTx,
    Preset(&'static str, &'static [u8]),
    Custom(CompactString, Vec<u8>),
}

impl MacroTxLineEnding {
    pub fn as_bytes<'a>(&'a self, rx: &'a RxLineEnding, tx: &'a TxLineEnding) -> &'a [u8] {
        match self {
            MacroTxLineEnding::InheritRx => rx.as_bytes(),
            MacroTxLineEnding::InheritTx => tx.as_bytes(rx),
            MacroTxLineEnding::Preset(_, preset) => preset,
            MacroTxLineEnding::Custom(_, custom) => custom,
        }
    }
}

impl std::fmt::Display for MacroTxLineEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InheritRx => write!(f, "InheritRX"),
            Self::InheritTx => write!(f, "InheritTX"),
            Self::Preset(preset, _) => write!(f, "{preset}"),
            Self::Custom(custom, _) => write!(f, "{custom}"),
        }
    }
}

impl FromStr for MacroTxLineEnding {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            _ if s.eq_ignore_ascii_case("InheritRX") => Ok(Self::InheritRx),
            _ if s.eq_ignore_ascii_case("InheritTX") => Ok(Self::InheritTx),

            escaped if PRESET_LINE_ENDINGS_ESCAPED.contains(&escaped) => {
                let index = PRESET_LINE_ENDINGS_ESCAPED
                    .iter()
                    .position(|le| *le == escaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            unescaped if PRESET_LINE_ENDINGS.contains(&unescaped) => {
                let index = PRESET_LINE_ENDINGS
                    .iter()
                    .position(|le| *le == unescaped)
                    .unwrap();

                Ok(Self::Preset(
                    PRESET_LINE_ENDINGS_ESCAPED[index],
                    PRESET_LINE_ENDINGS[index].as_bytes(),
                ))
            }
            other => {
                let into_bytes = Vec::unescape_bytes(other);
                Ok(Self::Custom(
                    into_bytes.escape_bytes().to_compact_string(),
                    into_bytes,
                ))
            }
        }
    }
}

impl<S: AsRef<str>> From<S> for RxLineEnding {
    fn from(value: S) -> Self {
        Self::from_str(value.as_ref()).unwrap()
    }
}
impl<S: AsRef<str>> From<S> for TxLineEnding {
    fn from(value: S) -> Self {
        Self::from_str(value.as_ref()).unwrap()
    }
}
impl<S: AsRef<str>> From<S> for MacroTxLineEnding {
    fn from(value: S) -> Self {
        Self::from_str(value.as_ref()).unwrap()
    }
}
