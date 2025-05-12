use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
};

use bstr::ByteVec;
use compact_str::CompactString;
use crokey::KeyCombination;
use indexmap::IndexMap;
use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    text::Text,
    widgets::{Cell, HighlightSpacing, Row, ScrollbarState, Table},
};
use tui_input::Input;

use crate::{
    keybinds::Keybinds, traits::HasEscapedBytes, tui::single_line_selector::SingleLineSelectorState,
};

mod macro_ref;
mod tui;

pub use macro_ref::MacroNameTag;
// pub use tui::{MacroEditSelected, MacroEditing};

// #[derive(Debug)]
// #[repr(u8)]
// pub enum MacrosPrompt {
//     None,
//     Delete,
//     AddEdit(MacroEditing),
// }

pub enum MacroCategorySelection<'a> {
    AllMacros,
    StringsOnly,
    WithBytes,
    NoCategory,
    Category(&'a str),
}

// TODO search when typing

pub struct Macros {
    pub all: BTreeMap<MacroNameTag, MacroString>,

    // pub ui_state: MacrosPrompt,
    // ["All Bytes", "All Strings", "All Macros", "OpenShock"]
    //     Start here, at user's first category.  ^
    pub categories_selector: SingleLineSelectorState,
    // TODO search
    pub search_input: Input,
    // pub scrollbar_state: ScrollbarState,
    // // maybe just take from macros
    // pub categories: BTreeSet<String>,
}

impl Macros {
    pub fn new() -> Self {
        // TODO Load from disk
        let mut test_macros = BTreeMap::new();

        test_macros.insert(
            MacroNameTag {
                title: "Backspace".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("\\x08"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Mrow!".into(),
                category: None,
            },
            MacroString::new("mrow"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Mrowwww".into(),
                category: None,
            },
            MacroString::new("mrowwww"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Version".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("version"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Factory Reset".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new("factoryreset"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Restart".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("restart"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "System Info".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("sysinfo"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Echo Off".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new("echo false"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Keep-Alive Off".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new("keepalive false"),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Setup Authtoken".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new("authtoken "),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Setup Networks".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new("networks "),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Get Config (JSON)".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("jsonconfig "),
        );
        test_macros.insert(
            MacroNameTag {
                title: "Get Config (Raw)".into(),
                category: Some("OpenShock".into()),
            },
            MacroString::new("rawconfig "),
        );
        test_macros.insert(
            MacroNameTag {
                title: "CaiX Vib (ID 12345, 0.5s)".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new(r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":500}"#),
        );
        test_macros.insert(
            MacroNameTag {
                title: "CaiX Vib (ID 12345, 1s)".into(),
                category: Some("OpenShock Setup".into()),
            },
            MacroString::new(r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":1000}"#),
        );

        Self {
            // scrollbar_state: ScrollbarState::new(test_macros.len()),
            all: test_macros,
            // tx_queue: Vec::new(),
            // ui_state: MacrosPrompt::None,
            search_input: Input::default(),
            categories_selector: SingleLineSelectorState::new().with_selected(2),
            // categories: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
    pub fn visible_len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.category_filtered_macros().count()
        }
    }
    pub fn none_visible(&self) -> bool {
        self.visible_len() == 0
    }
    fn selected_category(&self) -> MacroCategorySelection {
        match self.categories_selector.current_index {
            0 => MacroCategorySelection::WithBytes,
            1 => MacroCategorySelection::StringsOnly,
            2 => MacroCategorySelection::AllMacros,
            3 if self.has_no_category_macros() => MacroCategorySelection::NoCategory,
            index => MacroCategorySelection::Category(
                self.categories().nth(index - 3).unwrap_or("?????"),
            ),
        }
    }
    pub fn category_filtered_macros(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&MacroNameTag, &MacroString)> {
        let category = self.selected_category();

        self.all.iter().filter(move |(tag, string)| match category {
            MacroCategorySelection::AllMacros => true,
            MacroCategorySelection::StringsOnly => !string.has_bytes,
            MacroCategorySelection::WithBytes => string.has_bytes,
            MacroCategorySelection::NoCategory => tag.category.is_none(),
            MacroCategorySelection::Category(cat) => tag.category.as_deref() == Some(cat),
        })
    }
    pub fn as_table(&self, keybinds: &Keybinds, fuzzy_macro_name_match: bool) -> Table<'_> {
        let filtered = self
            .category_filtered_macros()
            .map(|m| (m.0.title.as_str(), m))
            .map(|(title, (tag, string))| {
                let macro_string = keybinds
                    .macros
                    .iter()
                    .filter(|(kc, km)| km.len() == 1)
                    .map(|(kc, km)| (kc, &km[0]))
                    .find(|(kc, km)| *km == tag)
                    .or_else(|| {
                        if fuzzy_macro_name_match {
                            keybinds
                                .macros
                                .iter()
                                .filter(|(kc, km)| km.len() == 1)
                                .map(|(kc, km)| (kc, &km[0]))
                                .find(|(kc, km)| km.eq_fuzzy(tag))
                        } else {
                            None
                        }
                    });
                let macro_string = macro_string.map(|(kc, km)| kc.to_string());

                (title, macro_string.unwrap_or_default())
            })
            .map(|(m, k)| Row::new([Text::raw(m), Text::raw(k).italic()]));

        let widths = [Constraint::Fill(4), Constraint::Fill(1)];
        let table = Table::new(filtered, widths).row_highlight_style(Style::new().reversed());
        table
    }
    pub fn has_no_category_macros(&self) -> bool {
        self.all.iter().any(|(tag, string)| tag.category.is_none())
    }
    pub fn categories<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a str> {
        let no_category = std::iter::once("No Category").filter(|_| self.has_no_category_macros());

        let categories: BTreeSet<&str> = self
            .all
            .iter()
            .filter_map(|(tag, string)| tag.category.as_deref())
            .collect();

        no_category.chain(categories.into_iter())
    }
    pub fn macro_from_key_combo<'a>(
        &'a self,
        key_combo: KeyCombination,
        macro_keybinds: &'a IndexMap<KeyCombination, Vec<MacroNameTag>>,
        fuzzy_macro_name_match: bool,
    ) -> Result<Vec<(&'a MacroNameTag, &'a MacroString)>, Option<Vec<&'a MacroNameTag>>> {
        // ) -> Result<Vec<usize>, Option<Vec<&KeybindMacro>>> {
        let Some(v) = macro_keybinds.get(&key_combo) else {
            return Err(None);
        };

        let mut somes: Vec<(&MacroNameTag, &MacroString)> = Vec::new();
        let mut nones: Vec<&MacroNameTag> = Vec::new();

        v.iter().for_each(|config_tag| {
            let eq_result = self
                .all
                .iter()
                // .enumerate()
                .find(|(tag, string)| config_tag.eq(tag));
            match eq_result {
                Some(macro_ref) => {
                    somes.push(macro_ref);
                    return;
                }
                None if fuzzy_macro_name_match => (),
                None => {
                    nones.push(config_tag);
                    return;
                }
            }
            assert!(fuzzy_macro_name_match);
            let eq_result = self
                .all
                .iter()
                // .enumerate()
                .find(|(tag, string)| config_tag.eq_fuzzy(tag));
            match eq_result {
                Some(macro_ref) => somes.push(macro_ref),
                None => nones.push(config_tag),
            }
        });

        if nones.is_empty() {
            Ok(somes)
        } else {
            Err(Some(nones))
        }
    }

    pub fn remove_macro(&mut self, macro_ref: &MacroNameTag) {
        self.all
            .remove(macro_ref)
            .expect("attempted removal of non-existant element");
    }
}

// #[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
// pub struct OwnedMacro {
//     pub title: CompactString,
//     pub category: Option<CompactString>,
//     // pub keybinding: Option<KeyCombination>,
//     pub content: MacroString,
//     // preview_hidden: bool,
// }

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacroString {
    pub inner: CompactString,
    pub has_bytes: bool,
} //maybe have has_bytes? dunno yet

impl MacroString {
    pub fn new<S: AsRef<str>>(inner: S) -> Self {
        Self {
            has_bytes: inner.as_ref().has_escaped_bytes(),
            inner: inner.as_ref().into(),
        }
    }
    pub fn update(&mut self, new: CompactString) {
        self.inner = new;
        self.has_bytes = self.inner.has_escaped_bytes();
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub fn unescape_bytes(&self) -> Vec<u8> {
        Vec::unescape_bytes(&self.inner)
    }
    pub fn as_str(&self) -> &str {
        &self.inner
    }
}

// impl OwnedMacro {
//     pub fn as_str(&self) -> Option<Cow<'_, str>> {
//         match &self.content {
//             MacroContent::Empty => None,
//             MacroContent::Text(text) => Some(Cow::Borrowed(text.as_str())),
//             MacroContent::Bytes { content, .. } => match std::str::from_utf8(content) {
//                 Ok(s) => Some(Cow::Borrowed(s)),
//                 Err(_) => None,
//             },
//         }
//     }
//     pub fn as_bytes(&self) -> Option<&[u8]> {
//         match &self.content {
//             MacroContent::Empty => None,
//             MacroContent::Bytes { content, .. } => Some(&content),
//             MacroContent::Text(text) => Some(text.as_bytes()),
//         }
//     }
// }

// impl fmt::Display for OwnedMacro {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         match &self.category {
//             Some(category) => write!(f, "{category} - {}", self.title),
//             None => write!(f, "{}", self.title),
//         }
//     }
// }

// impl OwnedMacro {
//     pub fn new_bytes<T: AsRef<str>>(
//         title: T,
//         category: Option<T>,
//         bytes: Vec<u8>,
//         // keybinding: Option<u8>,
//     ) -> Self {
//         Self {
//             title: title.as_ref().into(),
//             category: category.map(|t| t.as_ref().into()),
//             content: MacroContent::new_bytes(bytes),
//             // keybinding,
//         }
//     }
//     pub fn new_empty<T: AsRef<str>>(title: T) -> Self {
//         Self {
//             title: title.as_ref().into(),
//             category: None,
//             content: MacroContent::Empty,
//             // keybinding: None,
//         }
//     }
//     pub fn new_string<T: AsRef<str>, S: AsRef<str>>(
//         title: T,
//         category: Option<T>,
//         s: S,
//         // keybinding: Option<u8>,
//     ) -> Self {
//         Self {
//             title: title.as_ref().into(),
//             category: category.map(|t| t.as_ref().into()),
//             content: MacroContent::Text(s.as_ref().into()),
//             // keybinding,
//         }
//     }
//     pub fn preview(&self) -> &str {
//         match &self.content {
//             MacroContent::Text(text) => text.as_str(),
//             MacroContent::Bytes { preview, .. } => preview.as_str(),
//             MacroContent::Empty => "Empty! Please edit with `IDK YET`",
//         }
//     }
// }
