use std::fmt;
use std::str::FromStr;
use std::{collections::HashMap, time::Duration};

use compact_str::{CompactString, ToCompactString};
use crokey::KeyCombination;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::macros::MacroNameTag;

// enum ShowPopupAction {
//     #[strum(serialize = "show-portsettings")]
//     ShowPortSettings,
//     ShowBehavior,
//     ShowRendering,
//     #[strum(serialize = "show-macros")]
//     ShowPopup,
//     #[strum(serialize = "show-espflash")]
//     ShowPopup,
//     #[strum(serialize = "show-logging")]
//     ShowPopup,
// }

#[derive(Debug, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum BaseAction {
    ToggleTextwrap,
    ToggleTimestamps,
    ToggleIndices,
    ToggleHex,
    ToggleHexHeader,

    #[strum(serialize = "show-portsettings")]
    ShowPortSettings,
    ShowBehavior,
    ShowRendering,

    ReloadColors,
}

#[derive(Debug, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum PortAction {
    ToggleDtr,
    ToggleRts,
    AssertRts,
    DeassertRts,
    AssertDtr,
    DeassertDtr,
}

#[cfg(feature = "macros")]
#[derive(Debug, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum MacroAction {
    #[strum(serialize = "show-macros")]
    ShowPopup,

    ReloadMacros,
}

#[cfg(feature = "espflash")]
#[derive(Debug, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum EspAction {
    #[strum(serialize = "show-espflash")]
    ShowPopup,
    #[strum(serialize = "reload-espflash")]
    ReloadProfiles,
    EspHardReset,
    EspBootloader,
    EspBootloaderUnchecked,
    EspDeviceInfo,
    EspEraseFlash,
}

#[cfg(feature = "logging")]
#[derive(Debug, strum::EnumString)]
// #[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum LoggingAction {
    #[strum(serialize = "show-logging")]
    ShowPopup,
    #[strum(serialize = "logging-start")]
    Start,
    #[strum(serialize = "logging-stop")]
    Stop,
    #[strum(serialize = "logging-toggle")]
    Toggle,
}

#[derive(Debug)]
pub enum AppAction {
    Base(BaseAction),
    Port(PortAction),
    #[cfg(feature = "macros")]
    Macros(MacroAction),
    #[cfg(feature = "espflash")]
    Esp(EspAction),
    #[cfg(feature = "logging")]
    Logging(LoggingAction),
}

// impl AppAction {
//     pub fn requires_port_connection(&self) -> bool {
//         match self {
//             AppAction::Base(BaseAction::ToggleDtr) => true,
//             AppAction::Base(BaseAction::ToggleRts) => true,

//             AppAction::Base(_) => false,

//             #[cfg(feature = "logging")]
//             AppAction::Logging(LoggingAction::Start) => true,
//             #[cfg(feature = "logging")]
//             AppAction::Logging(LoggingAction::Toggle)
//             | AppAction::Logging(LoggingAction::Stop)
//             | AppAction::Logging(LoggingAction::ShowPopup) => false,
//             #[cfg(feature = "espflash")]
//             AppAction::Esp(EspAction::ShowPopup) | AppAction::Esp(EspAction::ReloadProfiles) => {
//                 false
//             }
//             #[cfg(feature = "espflash")]
//             AppAction::Esp(_) => true,
//             #[cfg(feature = "macros")]
//             AppAction::Macros(MacroAction::ReloadMacros)
//             | AppAction::Macros(MacroAction::ShowPopup) => false,
//         }
//     }
// }

impl From<BaseAction> for AppAction {
    fn from(action: BaseAction) -> Self {
        AppAction::Base(action)
    }
}

impl From<PortAction> for AppAction {
    fn from(action: PortAction) -> Self {
        AppAction::Port(action)
    }
}

#[cfg(feature = "macros")]
impl From<MacroAction> for AppAction {
    fn from(action: MacroAction) -> Self {
        AppAction::Macros(action)
    }
}

#[cfg(feature = "espflash")]
impl From<EspAction> for AppAction {
    fn from(action: EspAction) -> Self {
        AppAction::Esp(action)
    }
}

#[cfg(feature = "logging")]
impl From<LoggingAction> for AppAction {
    fn from(action: LoggingAction) -> Self {
        AppAction::Logging(action)
    }
}

impl FromStr for AppAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(base) = s.parse::<BaseAction>() {
            return Ok(AppAction::Base(base));
        }
        if let Ok(port) = s.parse::<PortAction>() {
            return Ok(AppAction::Port(port));
        }
        #[cfg(feature = "logging")]
        if let Ok(logging) = s.parse::<LoggingAction>() {
            return Ok(AppAction::Logging(logging));
        }
        #[cfg(feature = "macros")]
        if let Ok(macros) = s.parse::<MacroAction>() {
            return Ok(AppAction::Macros(macros));
        }
        #[cfg(feature = "espflash")]
        if let Ok(esp) = s.parse::<EspAction>() {
            return Ok(AppAction::Esp(esp));
        }

        Err(format!("Unrecognized AppAction variant for string: {}", s))
    }
}

#[derive(Debug)]
pub enum Action {
    AppAction(AppAction),
    EspFlashProfile(String),
    MacroInvocation(MacroNameTag),
    Pause(Duration),
}

// impl Action {
//     pub fn requires_port_connection(&self) -> bool {
//         match self {
//             Action::AppAction(action) => action.requires_port_connection(),
//             Action::EspFlashProfile(_) => true,
//             Action::MacroInvocation(_) => true,
//             Action::Pause(_) => true,
//         }
//     }
// }

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
shift-F21 = "esp-bootloader-unchecked"
ctrl-z = "esp-bootloader-unchecked"
ctrl-r = "show-rendering"
ctrl-h = "toggle-hex"
ctrl-l = "show-logging"

# macros
F19 = "Restart"
ctrl-s = "CaiX Vib (ID 12345, 0.5s)"
ctrl-g = "OpenShock Setup|Echo Off"
ctrl-j = ["PAUSE_MS:2000", "OpenShock Setup|Factory Reset", "PAUSE_MS:2000", "OpenShock Setup|Factory Reset"]

# espflash profiles
F18 = ["Core v2 1.4.0", "pause_ms:1000", "CaiX Vib (ID 12345, 1s)", "Echo Off", "Setup Authtoken and Networks"]
shift-F18 = ["esp-erase-flash", "Core v2 1.4.0"]
"#;

#[derive(Deserialize)]
pub struct Keybinds {
    #[serde(deserialize_with = "deserialize_keybinds_map")]
    pub keybindings: IndexMap<KeyCombination, Vec<String>>,
    #[serde(skip)]
    pub port_settings_hint: Option<CompactString>,
}

// fn serialize_macros_map<S>(
//     map: &IndexMap<KeyCombination, Vec<MacroNameTag>>,
//     serializer: S,
// ) -> Result<S::Ok, S::Error>
// where
//     S: serde::Serializer,
// {
//     use serde::ser::SerializeMap;
//     let mut map_ser = serializer.serialize_map(Some(map.len()))?;
//     for (k, v) in map {
//         let joined = v
//             .iter()
//             .map(|tag| tag.to_serialized_format())
//             .collect::<Vec<_>>()
//             .join(MACRO_CHAIN_DELIMITER);
//         map_ser.serialize_entry(k, &joined)?;
//     }
//     map_ser.end()
// }

fn deserialize_keybinds_map<'de, D>(
    deserializer: D,
) -> Result<IndexMap<KeyCombination, Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{MapAccess, Visitor};
    use std::fmt;

    struct KeybindsMapVisitor;

    impl<'de> Visitor<'de> for KeybindsMapVisitor {
        type Value = IndexMap<KeyCombination, Vec<String>>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map from key combination to delimited keybind actions")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = IndexMap::new();

            // Don't really need this type anywhere else, so it's fine being super private.
            #[derive(serde::Deserialize)]
            #[serde(untagged)]
            enum SingleOrSeveral<T> {
                Single(String),
                Several(Vec<T>),
            }

            while let Some((key, value)) =
                map.next_entry::<KeyCombination, SingleOrSeveral<String>>()?
            {
                let mut tags = match value {
                    SingleOrSeveral::Single(value) if value.trim().is_empty() => vec![],
                    SingleOrSeveral::Single(single) => vec![single],
                    SingleOrSeveral::Several(pre_split) => pre_split,
                    // SingleOrSeveral::Single(maybe_combined) => maybe_combined
                    //     .split(MACRO_CHAIN_DELIMITER_CHAR)
                    //     .map(str::to_string)
                    //     .collect::<Vec<String>>(),
                };

                // Remove any empty chain entries
                tags.retain(|tag| !tag.trim().is_empty());

                // And just don't bother remembering empty bindings.
                if tags.is_empty() {
                    continue;
                }

                result.insert(key, tags);
            }

            Ok(result)
        }
    }

    deserializer.deserialize_map(KeybindsMapVisitor)
}

impl Keybinds {
    pub fn new() -> Self {
        let mut deserialized: Self = toml::from_str(CONFIG_TOML).unwrap();

        deserialized.port_settings_hint = deserialized
            .keybindings
            .iter()
            .find(|(_, actions)| {
                if actions.len() != 1 {
                    return false;
                }

                if let Ok(AppAction::Base(BaseAction::ShowPortSettings)) =
                    actions[0].parse::<AppAction>()
                {
                    true
                } else {
                    false
                }
            })
            .map(|(kc, _)| kc.to_compact_string());

        deserialized
    }
    pub fn action_set_from_key_combo(&self, key_combo: KeyCombination) -> Option<&Vec<String>> {
        self.keybindings
            .iter()
            .find(|(kc, m)| *kc == &key_combo)
            .map(|(kc, m)| m)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_config_deser() {
        let keybinds = Keybinds::new();
        assert!(keybinds.keybindings.len() > 0);

        let port_settings_bind = keybinds.keybindings.get(&crokey::key!(ctrl - '.')).unwrap();
        assert_eq!(port_settings_bind[0], "show-portsettings");
        assert_eq!(
            keybinds
                .port_settings_hint
                .as_ref()
                .map(CompactString::as_str),
            Some("Ctrl-.")
        );
    }
}
