use std::fmt;

use compact_str::{CompactString, ToCompactString, format_compact};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacroNameTag {
    pub title: CompactString,
    pub category: Option<CompactString>,
}

// impl From<&OwnedMacro> for MacroNameTag {
//     fn from(value: &OwnedMacro) -> Self {
//         Self {
//             category: value.category.clone(),
//             title: value.title.clone(),
//         }
//     }
// }

// impl From<OwnedMacro> for MacroNameTag {
//     fn from(value: OwnedMacro) -> Self {
//         Self::from(&value)
//     }
// }

const MACRO_DELIMITER: char = '|';

impl fmt::Display for MacroNameTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref cat) = self.category {
            write!(f, "\"{}\" in \"{cat}\"", self.title)
        } else {
            write!(f, "\"{}\"", self.title)
        }
    }
}

impl MacroNameTag {
    // pub fn eq_macro(&self, other: &OwnedMacro) -> bool {
    //     self.title == other.title && self.category == other.category
    // }
    pub fn eq_fuzzy(&self, other: &MacroNameTag) -> bool {
        self.title == other.title
    }
}

// TODO allow chaining with ;

impl<'de> Deserialize<'de> for MacroNameTag {
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

        Ok(MacroNameTag { category, title })
    }
}

impl Serialize for MacroNameTag {
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
