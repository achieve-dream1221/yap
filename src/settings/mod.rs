use serde::{Deserialize, Serialize};

use crate::serial::PortSettings;

#[derive(Debug, Serialize, Deserialize)]
struct SettingsFile {}

struct Misc {}

pub struct Settings {}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Behavior {
    last_port_settings: PortSettings,
}
