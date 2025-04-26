use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use derivative::Derivative;
use fs_err::{self as fs};
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tracing::{info, level_filters::LevelFilter};

// Copied a lot from my other project, redefaulter
// https://github.com/nullstalgia/redefaulter/blob/ad81fad9468891b50daaac3215b0532386b6d1aa/src/settings/mod.rs

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
#[derive(Debug, Serialize, Deserialize, Derivative)]
#[derivative(Default)]
pub struct Behavior {
    #[serde(default)]
    pub last_port_settings: PortSettings,
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
