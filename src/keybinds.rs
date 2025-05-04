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
F19 = ["Restart"]
ctrl-r = ["Restart","Restart"]
ctrl-f = ["Cum|Factory Reset"]
ctrl-s = ["CaiX Vib (ID 12345, 0.5s)"]
ctrl-g = ["OpenShock Setup|Echo Off"]
ctrl-h = ["OpenShock Setup|Factory Reset","OpenShock Setup|Setup Authtoken","OpenShock Setup|Setup Networks"]
"#;

use serde::{Deserializer, Serializer};

use crate::macros::MacroRef;

// TODO use ; to chain macros

#[derive(Serialize, Deserialize)]
pub struct Keybinds {
    pub keybindings: HashMap<KeyCombination, String>,
    pub macros: HashMap<KeyCombination, Vec<MacroRef>>,
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
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_default_config_deser() {
        let keybinds: Keybinds = toml::from_str(CONFIG_TOML).unwrap();
    }
}
