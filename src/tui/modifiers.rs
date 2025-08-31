use std::str::FromStr;

use ratatui::style::Modifier;

#[derive(Debug, Default)]
pub struct ModifierFromStr {
    inner: Modifier,
}

impl FromStr for ModifierFromStr {
    type Err = UnrecognizedModifier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let separated = s.split(&[';', ',', ' ', '|', '-']);
        let mut combined_modifier = Modifier::default();

        for str in separated {
            let upper = str.to_ascii_uppercase();
            if let Some(parsed) = Modifier::from_name(&upper) {
                combined_modifier.insert(parsed);
            } else {
                return Err(UnrecognizedModifier);
            }
        }

        Ok(ModifierFromStr {
            inner: combined_modifier,
        })
    }
}

impl From<ModifierFromStr> for Modifier {
    fn from(value: ModifierFromStr) -> Self {
        value.inner
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unrecognized text modifier")]
pub struct UnrecognizedModifier;
