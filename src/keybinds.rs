use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;
use std::{collections::HashMap, time::Duration};

use compact_str::{CompactString, ToCompactString};
use crokey::KeyCombination;
use fs_err as fs;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[cfg(feature = "macros")]
use crate::macros::MacroNameTag;

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
#[repr(u8)]
// #[strum(prefix = "show-")]
// nevermind, doesn't work with FromStr :(
pub enum ShowPopupAction {
    #[strum(serialize = "show-portsettings")]
    ShowPortSettings,
    ShowBehavior,
    ShowRendering,
    #[cfg(feature = "macros")]
    ShowMacros,
    #[cfg(feature = "espflash")]
    #[strum(serialize = "show-espflash")]
    ShowEspFlash,
    #[cfg(feature = "logging")]
    ShowLogging,
    #[cfg(feature = "defmt")]
    ShowDefmt,
}

// impl From<ShowPopupAction> for PopupMenu {
//     fn from(value: ShowPopupAction) -> Self {
//         match value {
//             ShowPopupAction::ShowBehavior => Self::BehaviorSettings,
//             ShowPopupAction::ShowRendering => Self::RenderingSettings,
//             ShowPopupAction::ShowPortSettings => Self::PortSettings,
//             #[cfg(feature = "logging")]
//             ShowPopupAction::ShowLogging => Self::Logging,
//             #[cfg(feature = "espflash")]
//             ShowPopupAction::ShowEspFlash => Self::EspFlash,
//             #[cfg(feature = "macros")]
//             ShowPopupAction::ShowMacros => Self::Macros,
//             #[cfg(feature = "defmt")]
//             ShowPopupAction::ShowDefmt => Self::Defmt,
//         }
//     }
// }

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum BaseAction {
    ToggleTextwrap,
    ToggleTimestamps,
    ToggleIndices,
    ToggleHex,
    ToggleHexHeader,

    ShowKeybinds,

    ReloadColors,
}

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum PortAction {
    ToggleDtr,
    ToggleRts,
    AssertRts,
    DeassertRts,
    AssertDtr,
    DeassertDtr,
    AttemptReconnectStrict,
    AttemptReconnectLoose,
}

#[cfg(feature = "macros")]
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum MacroAction {
    ReloadMacros,
}

#[cfg(feature = "espflash")]
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum EspAction {
    #[strum(serialize = "reload-espflash")]
    ReloadProfiles,
    EspHardReset,
    EspBootloader,
    EspBootloaderUnchecked,
    EspDeviceInfo,
    EspEraseFlash,
}

#[cfg(feature = "logging")]
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
// #[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
pub enum LoggingAction {
    #[strum(serialize = "logging-start")]
    Start,
    #[strum(serialize = "logging-stop")]
    Stop,
    #[strum(serialize = "logging-toggle")]
    Toggle,
}

#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, strum::EnumString, strum::Display, strum::AsRefStr,
)]
#[strum(serialize_all = "kebab-case")]
#[strum(ascii_case_insensitive)]
#[repr(u8)]
// #[strum(prefix = "show-")]
// nevermind, doesn't work with FromStr :(
pub enum ShowDefmtSelect {
    #[strum(serialize = "defmt-select-tui")]
    SelectTui,
    #[strum(serialize = "defmt-select-system")]
    SelectSystem,
    #[strum(serialize = "defmt-select-recent")]
    SelectRecent,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, strum::AsRefStr)]
pub enum AppAction {
    Base(BaseAction),
    Popup(ShowPopupAction),
    Port(PortAction),
    #[cfg(feature = "macros")]
    Macros(MacroAction),
    #[cfg(feature = "espflash")]
    Esp(EspAction),
    #[cfg(feature = "logging")]
    Logging(LoggingAction),
    #[cfg(feature = "defmt")]
    ShowDefmtSelect(ShowDefmtSelect),
}

// // TODO replace this
// impl AppAction {
//     pub fn discriminant(&self) -> u8 {
//         match self {
//             Self::Base(_) => 0,
//             Self::Popup(_) => 1,
//             Self::Port(_) => 2,
//             #[cfg(feature = "macros")]
//             Self::Macros(_) => 3,
//             #[cfg(feature = "espflash")]
//             Self::Esp(_) => 5,
//             #[cfg(feature = "logging")]
//             Self::Logging(_) => 6,
//         }
//     }
// }

impl fmt::Display for AppAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppAction::Base(action) => write!(f, "{action}"),
            AppAction::Popup(action) => write!(f, "{action}"),
            AppAction::Port(action) => write!(f, "{action}"),
            #[cfg(feature = "macros")]
            AppAction::Macros(action) => write!(f, "{action}"),
            #[cfg(feature = "espflash")]
            AppAction::Esp(action) => write!(f, "{action}"),
            #[cfg(feature = "logging")]
            AppAction::Logging(action) => write!(f, "{action}"),
            #[cfg(feature = "defmt")]
            AppAction::ShowDefmtSelect(action) => write!(f, "{action}"),
        }
    }
}

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

#[cfg(feature = "defmt")]
impl From<ShowDefmtSelect> for AppAction {
    fn from(action: ShowDefmtSelect) -> Self {
        AppAction::ShowDefmtSelect(action)
    }
}

impl FromStr for AppAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(base) = s.parse::<BaseAction>() {
            return Ok(AppAction::Base(base));
        }
        if let Ok(popup) = s.parse::<ShowPopupAction>() {
            return Ok(AppAction::Popup(popup));
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
        #[cfg(feature = "defmt")]
        if let Ok(defmt_select) = s.parse::<ShowDefmtSelect>() {
            return Ok(AppAction::ShowDefmtSelect(defmt_select));
        }

        Err(format!("Unrecognized AppAction variant for string: {}", s))
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    AppAction(AppAction),
    #[cfg(feature = "espflash")]
    EspFlashProfile(String),
    #[cfg(feature = "macros")]
    MacroInvocation(MacroNameTag),
    Pause(Duration),
}

static OVERRIDABLE_DEFAULTS: &str = r#"
[keybindings]
ctrl-o = "toggle-dtr"
ctrl-p = "toggle-rts"

ctrl-w = "toggle-textwrap"
ctrl-t = "toggle-timestamps"
ctrl-d = "toggle-indices"

ctrl-b = "show-behavior"
'ctrl-.' = "show-portsettings"

ctrl-f = "reload-colors"
ctrl-h = "show-keybinds"
'ctrl-/' = "show-keybinds"
"#;

pub const CONFIG_TOML_PATH: &str = "yap_keybinds.toml";

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
    pub fn fill_port_settings_hint(&mut self) {
        self.port_settings_hint = self
            .keybindings
            .iter()
            .find(|(_, actions)| {
                if actions.len() != 1 {
                    return false;
                }

                if let Ok(AppAction::Popup(ShowPopupAction::ShowPortSettings)) =
                    actions[0].parse::<AppAction>()
                {
                    true
                } else {
                    false
                }
            })
            .map(|(kc, _)| kc.to_compact_string());
    }
    pub fn overridable_defaults() -> Self {
        let mut deserialized: Self = toml::from_str(OVERRIDABLE_DEFAULTS).unwrap();

        deserialized.fill_port_settings_hint();

        deserialized
    }
    pub fn load(input: &str) -> Result<Self, toml::de::Error> {
        let mut overridable = Self::overridable_defaults();

        let user_settings: Self = toml::from_str(input)?;

        overridable
            .keybindings
            .extend(user_settings.keybindings.into_iter());

        overridable.fill_port_settings_hint();

        Ok(overridable)
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
        let keybinds =
            Keybinds::load(include_str!("../example_configs/yap_keybinds.toml")).unwrap();
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
