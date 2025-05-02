use std::collections::HashMap;
use std::fmt;

use crokey::KeyCombination;
use serde::{Deserialize, Serialize};

pub const TOGGLE_TEXTWRAP: &str = "toggle-textwrap";
pub const TOGGLE_DTR: &str = "toggle-dtr";
pub const TOGGLE_RTS: &str = "toggle-rts";
pub const TOGGLE_TIMESTAMPS: &str = "toggle-timestamps";
pub const SHOW_MACROS: &str = "show-macros";
pub const SHOW_PORTSETTINGS: &str = "show-portsettings";

static CONFIG_TOML: &str = r#"
[keybindings]
ctrl-w = "toggle-textwrap"
ctrl-o = "toggle-dtr"
ctrl-p = "toggle-rts"
ctrl-e = "esp-bootloader"
ctrl-t = "toggle-timestamps"
ctrl-m = "show-macros"
'ctrl-.' = "show-portsettings"

[macros]
ctrl-h = "meow"
ctrl-r = "Restart"
F19 = "Restart"
ctrl-f = "Cum|Factory Reset"
ctrl-s = "CaiX Vib (ID 12345, 0.5s)"
ctrl-g = "OpenShock Setup|Echo Off"
"#;

use serde::{Deserializer, Serializer};

use crate::macros::Macro;

// TODO use ; to chain macros

const MACRO_DELIMITER: char = '|';

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeybindMacro {
    pub category: Option<String>,
    pub title: String,
}

impl fmt::Display for KeybindMacro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref cat) = self.category {
            write!(f, "\"{}\" in \"{cat}\"", self.title)
        } else {
            write!(f, "\"{}\"", self.title)
        }
    }
}

impl KeybindMacro {
    pub fn eq_macro(&self, other: &Macro) -> bool {
        self.title == other.title && self.category == other.category
    }
    pub fn eq_macro_fuzzy(&self, other: &Macro) -> bool {
        self.title == other.title
    }
}

impl<'de> Deserialize<'de> for KeybindMacro {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let buf = String::deserialize(deserializer)?;
        let parts: Vec<&str> = buf.splitn(2, MACRO_DELIMITER).collect();

        let (category, title) = match parts.len() {
            2 => (
                Some(parts[0].trim().to_string()),
                parts[1].trim().to_string(),
            ),
            1 => (None, parts[0].trim().to_string()),
            _ => (None, String::new()),
        };

        Ok(KeybindMacro { category, title })
    }
}

impl Serialize for KeybindMacro {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = if let Some(ref cat) = self.category {
            format!("{}|{}", cat, self.title)
        } else {
            self.title.clone()
        };
        serializer.serialize_str(&s)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Keybinds {
    pub keybindings: HashMap<KeyCombination, String>,
    // #[serde(
    //     deserialize_with = "deserialize_keybind_macros",
    //     serialize_with = "serialize_keybind_macros"
    // )]
    pub macros: HashMap<KeyCombination, KeybindMacro>,
}

impl Keybinds {
    pub fn new() -> Self {
        toml::from_str(CONFIG_TOML).unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_config_deser() {
        toml::from_str(CONFIG_TOML).unwrap()
    }
}
