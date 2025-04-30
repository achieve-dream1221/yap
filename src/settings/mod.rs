use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use derivative::Derivative;
use fs_err::{self as fs};
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
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

use crate::serial::PortSettings;

pub mod ser;

#[serde_inline_default]
#[derive(Debug, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct Settings {
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(default)]
    pub behavior: Behavior,
    #[serde(default)]
    pub misc: Misc,
    #[serde(default)]
    pub last_port_settings: PortSettings,
}

#[serde_inline_default]
#[derive(Debug, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct Misc {
    #[serde_inline_default(String::from("debug"))]
    #[derivative(Default(value = "String::from(\"debug\")"))]
    pub log_level: String,
}

#[serde_inline_default]
#[derive(Debug, Clone, Serialize, Deserialize, StructTable, Derivative)]
#[derivative(Default)]
pub struct Behavior {
    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Use text box to type in before sending, with history. If disabled, sends keyboard inputs directly (TODO).
    pub fake_shell: bool,

    #[serde(default)]
    /// Persist Fake Shell's command history across sessions (TODO).
    pub retain_history: bool,

    #[serde_inline_default(true)]
    #[derivative(Default(value = "true"))]
    /// Show user input in buffer after sending.
    pub echo_user_text: bool,

    #[serde(default)]
    /// Show timestamps next to each incoming line.
    pub timestamps: bool,
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
