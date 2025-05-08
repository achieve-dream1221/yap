use std::{
    collections::{BTreeSet, HashMap},
    fmt,
};

use compact_str::CompactString;
use crokey::KeyCombination;
use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    text::Text,
    widgets::{Cell, HighlightSpacing, Row, ScrollbarState, Table},
};
use tui_input::Input;

use crate::{keybinds::Keybinds, tui::single_line_selector::SingleLineSelectorState};

mod macro_ref;
mod tui;

pub use macro_ref::MacroRef;
pub use tui::MacroEditing;

#[derive(Debug)]
#[repr(u8)]
pub enum MacrosPrompt {
    None,
    AddEdit(MacroEditing),
    Delete,
    Keybind,
}

pub enum MacroCategorySelection<'a> {
    AllMacros,
    AllStrings,
    AllBytes,
    NoCategory,
    Category(&'a str),
}

// TODO search when typing

pub struct Macros {
    pub all: BTreeSet<Macro>,

    pub ui_state: MacrosPrompt,
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
        let test_macros = BTreeSet::from([
            Macro::new_string("Mrow!", None, "mrow"),
            Macro::new_empty("Mrowwww"),
            Macro::new_string("Version", Some("OpenShock"), "version"),
            Macro::new_string("Factory Reset", Some("OpenShock Setup"), "factoryreset"),
            Macro::new_string("Restart", Some("OpenShock"), "restart"),
            Macro::new_string("System Info", Some("OpenShock"), "sysinfo"),
            Macro::new_string("Echo Off", Some("OpenShock Setup"), "echo false"),
            Macro::new_string("Keep-Alive Off", Some("OpenShock Setup"), "keepalive false"),
            Macro::new_bytes(
                "Setup Networks",
                Some("OpenShock Setup"),
                br#"networks "#.into(),
            ),
            Macro::new_string("Setup Authtoken", Some("OpenShock Setup"), "authtoken "),
            Macro::new_string("Get Config (JSON)", Some("OpenShock"), "jsonconfig "),
            Macro::new_string("Get Config (Raw)", Some("OpenShock"), "rawconfig "),
            Macro::new_string(
                "CaiX Vib (ID 12345, 0.5s)",
                Some("OpenShock Setup"),
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":500}"#,
            ),
            Macro::new_string(
                "CaiX Vib (ID 12345, 1s)",
                Some("OpenShock Setup"),
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":1000}"#,
            ),
            Macro::new_bytes("Backspace", Some("OpenShock"), b"\x08".into()),
        ]);
        Self {
            // scrollbar_state: ScrollbarState::new(test_macros.len()),
            all: test_macros,
            // tx_queue: Vec::new(),
            ui_state: MacrosPrompt::None,
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
            0 => MacroCategorySelection::AllBytes,
            1 => MacroCategorySelection::AllStrings,
            2 => MacroCategorySelection::AllMacros,
            3 if self.has_no_category_macros() => MacroCategorySelection::NoCategory,
            index => MacroCategorySelection::Category(
                self.categories().nth(index - 3).unwrap_or("?????"),
            ),
        }
    }
    pub fn category_filtered_macros(&self) -> impl DoubleEndedIterator<Item = &Macro> {
        let category = self.selected_category();

        self.all.iter().filter(move |m| match category {
            MacroCategorySelection::AllMacros => true,
            MacroCategorySelection::AllStrings => matches!(m.content, MacroContent::Text(_)),
            MacroCategorySelection::AllBytes => matches!(m.content, MacroContent::Bytes { .. }),
            MacroCategorySelection::NoCategory => m.category.is_none(),
            MacroCategorySelection::Category(cat) => m.category.as_deref() == Some(cat),
        })
    }
    pub fn as_table(&self, keybinds: &Keybinds, fuzzy_macro_name_match: bool) -> Table<'_> {
        let filtered = self
            .category_filtered_macros()
            .map(|m| (m.title.as_str(), m))
            .map(|(title, m)| {
                let macro_string = keybinds
                    .macros
                    .iter()
                    .filter(|(kc, km)| km.len() == 1)
                    .map(|(kc, km)| (kc, &km[0]))
                    .find(|(kc, km)| km.eq_macro(m))
                    .or_else(|| {
                        if fuzzy_macro_name_match {
                            keybinds
                                .macros
                                .iter()
                                .filter(|(kc, km)| km.len() == 1)
                                .map(|(kc, km)| (kc, &km[0]))
                                .find(|(kc, km)| km.eq_macro_fuzzy(m))
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
        self.all.iter().any(|m| m.category.is_none())
    }
    pub fn categories<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a str> {
        let no_category = std::iter::once("No Category").filter(|_| self.has_no_category_macros());

        let categories: BTreeSet<&str> = self
            .all
            .iter()
            .filter_map(|m| m.category.as_deref())
            .collect();

        no_category.chain(categories.into_iter())
    }
    pub fn macro_from_key_combo<'a>(
        &'a self,
        key_combo: KeyCombination,
        macro_keybinds: &'a HashMap<KeyCombination, Vec<MacroRef>>,
        fuzzy_macro_name_match: bool,
    ) -> Result<Vec<&'a Macro>, Option<Vec<&'a MacroRef>>> {
        // ) -> Result<Vec<usize>, Option<Vec<&KeybindMacro>>> {
        let Some(v) = macro_keybinds.get(&key_combo) else {
            return Err(None);
        };

        let mut somes: Vec<&Macro> = Vec::new();
        let mut nones: Vec<&MacroRef> = Vec::new();

        v.iter().for_each(|km| {
            let eq_result = self
                .all
                .iter()
                // .enumerate()
                .find(|m| km.eq_macro(m));
            match eq_result {
                Some(macro_ref) => {
                    somes.push(macro_ref);
                    return;
                }
                None if fuzzy_macro_name_match => (),
                None => {
                    nones.push(km);
                    return;
                }
            }
            assert!(fuzzy_macro_name_match);
            let eq_result = self
                .all
                .iter()
                // .enumerate()
                .find(|m| km.eq_macro_fuzzy(m));
            match eq_result {
                Some(macro_ref) => somes.push(macro_ref),
                None => nones.push(km),
            }
        });

        if nones.is_empty() {
            Ok(somes)
        } else {
            Err(Some(nones))
        }
    }

    pub fn remove_macro(&mut self, macro_ref: &Macro) {
        self.all.take(macro_ref).expect("expected removal of macro");

        // let macro_binding = self.all.iter().find(|d| macro_ref.eq_macro(d)).unwrap();
        // self.all.remove(macro_binding);
    }

    pub fn remove_macro_by_ref(&mut self, macro_ref: &MacroRef) {
        let orig_len = self.all.len();
        self.all.retain(|m| !macro_ref.eq_macro(m));
        assert_eq!(
            self.all.len(),
            orig_len - 1,
            "expected the removal of exactly one element"
        );

        // let macro_binding = self.all.iter().find(|d| macro_ref.eq_macro(d)).unwrap();
        // self.all.remove(macro_binding);
    }
    // pub fn begin_editing(&mut self, macro_ref: &MacroRef) {
    //     self.ui_state = MacrosPrompt::AddEdit(MacroEditing {
    //         inner_ref: Some(macro_ref.clone()),
    //         ..Default::default()
    //     });

    // }
}

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Macro {
    pub title: CompactString,
    pub category: Option<CompactString>,
    // pub keybinding: Option<KeyCombination>,
    pub content: MacroContent,
    // preview_hidden: bool,
}

impl fmt::Display for Macro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.category {
            Some(category) => write!(f, "{category} - {}", self.title),
            None => write!(f, "{}", self.title),
        }
    }
}

// // Custom Eq+Ord impls to avoid checking `content` when sorting.
// impl PartialEq for Macro {
//     fn eq(&self, other: &Self) -> bool {
//         self.title == other.title && self.keybinding == other.keybinding
//     }
// }

// impl Eq for Macro {}

// impl PartialOrd for Macro {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         match self.title.partial_cmp(&other.title) {
//             Some(std::cmp::Ordering::Equal) => self.keybinding.partial_cmp(&other.keybinding),
//             ord => ord,
//         }
//     }
// }

// impl Ord for Macro {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         match self.title.cmp(&other.title) {
//             std::cmp::Ordering::Equal => self.keybinding.cmp(&other.keybinding),
//             ord => ord,
//         }
//     }
// }

#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacroContent {
    #[default]
    Empty,
    Text(String),
    Bytes {
        content: Vec<u8>,
        preview: String,
    },
}

impl MacroContent {
    fn new_bytes(content: Vec<u8>) -> Self {
        let hex_string = content
            .iter()
            .map(|b| format!("0x{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");
        Self::Bytes {
            content,
            preview: hex_string,
        }
    }
    // fn update_byte_preview(&mut self) {
    //     match self {
    //         MacroContent::Bytes { content, preview } => {
    //             *preview = content
    //                 .iter()
    //                 .map(|b| format!("{:02X}", b))
    //                 .collect::<Vec<_>>()
    //                 .join(" ")
    //         }
    //         _ => unreachable!(),
    //     }
    // }
}

impl Macro {
    pub fn new_bytes<T: AsRef<str>>(
        title: T,
        category: Option<T>,
        bytes: Vec<u8>,
        // keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
            category: category.map(|t| t.as_ref().into()),
            content: MacroContent::new_bytes(bytes),
            // keybinding,
        }
    }
    pub fn new_empty<T: AsRef<str>>(title: T) -> Self {
        Self {
            title: title.as_ref().into(),
            category: None,
            content: MacroContent::Empty,
            // keybinding: None,
        }
    }
    pub fn new_string<T: AsRef<str>, S: AsRef<str>>(
        title: T,
        category: Option<T>,
        s: S,
        // keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
            category: category.map(|t| t.as_ref().into()),
            content: MacroContent::Text(s.as_ref().into()),
            // keybinding,
        }
    }
    pub fn preview(&self) -> &str {
        match &self.content {
            MacroContent::Text(text) => text.as_str(),
            MacroContent::Bytes { preview, .. } => preview.as_str(),
            MacroContent::Empty => "Empty! Please edit with `IDK YET`",
        }
    }
}
