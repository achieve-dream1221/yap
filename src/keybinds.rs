use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crokey::KeyCombination;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const MACRO_CHAIN_DELIMITER: &str = "~";
pub const MACRO_CHAIN_DELIMITER_CHAR: char = '~';

pub mod methods {
    pub const TOGGLE_TEXTWRAP: &str = "toggle-textwrap";
    pub const TOGGLE_DTR: &str = "toggle-dtr";
    pub const TOGGLE_RTS: &str = "toggle-rts";
    pub const TOGGLE_TIMESTAMPS: &str = "toggle-timestamps";
    pub const TOGGLE_INDICES: &str = "toggle-indices";
    pub const SHOW_MACROS: &str = "show-macros";
    pub const SHOW_PORTSETTINGS: &str = "show-portsettings";
    pub const SHOW_BEHAVIOR: &str = "show-behavior";
    pub const SHOW_RENDERING: &str = "show-rendering";
    pub const RELOAD_MACROS: &str = "reload-macros";
    pub const RELOAD_COLORS: &str = "reload-colors";
}

#[cfg(feature = "espflash")]
pub mod esp_methods {
    pub const SHOW_ESPFLASH: &str = "show-espflash";
    pub const RELOAD_ESPFLASH: &str = "reload-espflash";
    pub const ESP_HARD_RESET: &str = "esp-hard-reset";
    pub const ESP_BOOTLOADER: &str = "esp-bootloader";
    pub const ESP_DEVICE_INFO: &str = "esp-device-info";
    pub const ESP_ERASE_FLASH: &str = "esp-erase-flash";
}

static CONFIG_TOML: &str = r#"
[keybindings]
ctrl-w = "toggle-textwrap"
ctrl-o = "toggle-dtr"
ctrl-p = "toggle-rts"
ctrl-e = "show-espflash"
# ctrl-e = "reload-macros"
# ctrl-e = "esp-bootloader"
ctrl-t = "toggle-timestamps"
ctrl-m = "show-macros"
ctrl-b = "show-behavior"
'ctrl-.' = "show-portsettings"
ctrl-d = "toggle-indices"
ctrl-f = "reload-colors"
F20 = "esp-hard-reset"
F21 = "esp-bootloader"

[macros]
F19 = "Restart"
ctrl-r = ["Restart","Restart"]
ctrl-f = "Cum|Factory Reset"
ctrl-s = "CaiX Vib (ID 12345, 0.5s)"
ctrl-g = "OpenShock Setup|Echo Off"
ctrl-h = "OpenShock Setup|Factory Reset~OpenShock Setup|Setup Authtoken~OpenShock Setup|Setup Networks"

[espflash_profiles]
F18 = "PIO Core v2"
"#;

use crate::macros::MacroNameTag;

// TODO use ; to chain macros

#[derive(Serialize, Deserialize)]
pub struct Keybinds {
    pub keybindings: IndexMap<KeyCombination, String>,
    #[serde(
        serialize_with = "serialize_macros_map",
        deserialize_with = "deserialize_macros_map"
    )]
    //
    pub macros: IndexMap<KeyCombination, Vec<MacroNameTag>>,

    #[cfg(feature = "espflash")]
    // #[serde(rename = "espflash.profiles")]
    pub espflash_profiles: IndexMap<KeyCombination, String>,
}

fn serialize_macros_map<S>(
    map: &IndexMap<KeyCombination, Vec<MacroNameTag>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map_ser = serializer.serialize_map(Some(map.len()))?;
    for (k, v) in map {
        let joined = v
            .iter()
            .map(|tag| tag.to_serialized_format())
            .collect::<Vec<_>>()
            .join(MACRO_CHAIN_DELIMITER);
        map_ser.serialize_entry(k, &joined)?;
    }
    map_ser.end()
}

fn deserialize_macros_map<'de, D>(
    deserializer: D,
) -> Result<IndexMap<KeyCombination, Vec<MacroNameTag>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{MapAccess, Visitor};
    use std::fmt;

    struct MacrosMapVisitor;

    impl<'de> Visitor<'de> for MacrosMapVisitor {
        type Value = IndexMap<KeyCombination, Vec<MacroNameTag>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map from key combination to delimited macro names")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = IndexMap::new();

            // Don't really need this type anywhere else, so it's fine being super private.
            #[derive(serde::Deserialize)]
            #[serde(untagged)]
            enum StringOrMacroArray {
                String(String),
                MacroArray(Vec<MacroNameTag>),
            }

            while let Some((key, value)) = map.next_entry::<KeyCombination, StringOrMacroArray>()? {
                let tags = match value {
                    StringOrMacroArray::String(maybe_combined) if maybe_combined.is_empty() => {
                        Vec::new()
                    }
                    StringOrMacroArray::String(maybe_combined) => maybe_combined
                        .split(MACRO_CHAIN_DELIMITER_CHAR)
                        .map(|s| {
                            MacroNameTag::from_str(s).map_err(|_| {
                                serde::de::Error::custom(format!("Invalid MacroNameTag: '{}'", s))
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,

                    StringOrMacroArray::MacroArray(pre_split) => pre_split,
                };

                result.insert(key, tags);
            }
            Ok(result)
        }
    }

    deserializer.deserialize_map(MacrosMapVisitor)
}

impl Keybinds {
    pub fn new() -> Self {
        toml::from_str(CONFIG_TOML).unwrap()
    }
    pub fn method_from_key_combo(&self, key_combo: KeyCombination) -> Option<&str> {
        self.keybindings
            .iter()
            .find(|(kc, m)| *kc == &key_combo)
            .map(|(kc, m)| m.as_str())
    }
    #[cfg(feature = "espflash")]
    pub fn espflash_profile_from_key_combo(&self, key_combo: KeyCombination) -> Option<&str> {
        self.espflash_profiles
            .iter()
            .find(|(kc, name)| *kc == &key_combo)
            .map(|(kc, name)| name.as_str())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_config_deser() {
        let keybinds: Keybinds = toml::from_str(CONFIG_TOML).unwrap();
        assert!(keybinds.keybindings.len() > 0);
        assert!(keybinds.macros.len() > 0);
    }
}
