use std::fmt;

use compact_str::{CompactString, ToCompactString, format_compact};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::Macro;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacroRef {
    pub category: Option<CompactString>,
    pub title: CompactString,
}

impl From<&Macro> for MacroRef {
    fn from(value: &Macro) -> Self {
        Self {
            category: value.category.clone(),
            title: value.title.clone(),
        }
    }
}

impl From<Macro> for MacroRef {
    fn from(value: Macro) -> Self {
        Self::from(&value)
    }
}

const MACRO_DELIMITER: char = '|';

impl fmt::Display for MacroRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref cat) = self.category {
            write!(f, "\"{}\" in \"{cat}\"", self.title)
        } else {
            write!(f, "\"{}\"", self.title)
        }
    }
}

impl MacroRef {
    pub fn eq_macro(&self, other: &Macro) -> bool {
        self.title == other.title && self.category == other.category
    }
    pub fn eq_macro_fuzzy(&self, other: &Macro) -> bool {
        self.title == other.title
    }
}

// TODO allow chaining with ;

impl<'de> Deserialize<'de> for MacroRef {
    /// Example inputs that can be deserialized:
    ///
    /// - `"lorem|ipsum"` → `MacroRef { category: Some("lorem".into()), title: "ipsum".into() }`
    /// - `"ipsum"`       → `MacroRef { category: None, title: "ipsum".into() }`
    /// - `"foo | bar"`   → `MacroRef { category: Some("foo".into()), title: "bar".into() }`
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let buf = String::deserialize(deserializer)?;
        let parts: Vec<&str> = buf.splitn(2, MACRO_DELIMITER).collect();

        let (category, title) = match parts.len() {
            2 => (
                Some(parts[0].trim().to_compact_string()),
                parts[1].trim().to_compact_string(),
            ),
            1 => (None, parts[0].trim().to_compact_string()),
            _ => return Err(serde::de::Error::custom("invalid format for MacroRef")),
        };

        Ok(MacroRef { category, title })
    }
}

impl Serialize for MacroRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = if let Some(ref cat) = self.category {
            format_compact!("{}|{}", cat, self.title)
        } else {
            self.title.clone()
        };
        serializer.serialize_str(&s)
    }
}
