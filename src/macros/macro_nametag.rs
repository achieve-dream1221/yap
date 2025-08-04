use std::fmt;

use compact_str::{CompactString, ToCompactString, format_compact};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacroNameTag {
    pub name: CompactString,
    /// Category is set to None if it would be empty.
    pub category: Option<CompactString>,
}

impl MacroNameTag {
    pub fn to_serialized_format(&self) -> CompactString {
        if let Some(cat) = &self.category {
            format_compact!("{}|{}", cat, self.name)
        } else {
            self.name.clone()
        }
    }
}

const MACRO_DELIMITER: char = '|';

impl fmt::Display for MacroNameTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref cat) = self.category {
            write!(f, "{cat}: {}", self.name)
        } else {
            write!(f, "{}", self.name)
        }
    }
}

impl MacroNameTag {
    // pub fn eq_macro(&self, other: &OwnedMacro) -> bool {
    //     self.title == other.title && self.category == other.category
    // }
    pub fn eq_fuzzy(&self, other: &MacroNameTag) -> bool {
        self.name == other.name
    }
}

impl std::str::FromStr for MacroNameTag {
    type Err = String;

    /// Example inputs that can be parsed:
    ///
    /// - `"lorem|ipsum"` → `MacroRef { category: Some("lorem".into()), title: "ipsum".into() }`
    /// - `"ipsum"`       → `MacroRef { category: None, title: "ipsum".into() }`
    /// - `"foo | bar"`   → `MacroRef { category: Some("foo".into()), title: "bar".into() }`
    fn from_str(buf: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = buf.splitn(2, MACRO_DELIMITER).collect();

        let (category, title) = match parts.len() {
            2 => (
                Some(parts[0].trim().to_compact_string()),
                parts[1].trim().to_compact_string(),
            ),
            1 => (None, parts[0].trim().to_compact_string()),
            _ => return Err("invalid format for MacroNameTag".to_owned()),
        };

        Ok(MacroNameTag {
            category,
            name: title,
        })
    }
}

impl<'de> Deserialize<'de> for MacroNameTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let buf = String::deserialize(deserializer)?;
        buf.parse().map_err(serde::de::Error::custom)
    }
}

impl Serialize for MacroNameTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_serialized_format())
    }
}
