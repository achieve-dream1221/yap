use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct SettingsFile {}

struct Misc {}

pub struct Settings {}

struct Behavior {
    last_baud: u32,
}
