use std::collections::BTreeSet;

use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    text::Text,
    widgets::{Cell, HighlightSpacing, Row, ScrollbarState, Table},
};
use tui_input::Input;

use crate::tui::single_line_selector::SingleLineSelectorState;

pub struct Macros {
    inner: BTreeSet<Macro>,
    pub ui_state: MacrosPrompt,
    // ["All Bytes", "All Strings", "All Macros", "OpenShock"]
    //     Start here, at user's first category.  ^
    pub categories_selector: SingleLineSelectorState,
    pub input: Input,
    // pub scrollbar_state: ScrollbarState,
    // // maybe just take from macros
    // pub categories: BTreeSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MacrosPrompt {
    None,
    Create,
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

impl Macros {
    pub fn new() -> Self {
        // TODO Load from disk
        let test_macros = BTreeSet::from([
            Macro::new_string("Mrow!", None, "mrow", None),
            Macro::new_string("Version", Some("OpenShock"), "version", None),
            Macro::new_string(
                "Factory Reset",
                Some("OpenShock Setup"),
                "factoryreset",
                None,
            ),
            Macro::new_string("Restart", Some("OpenShock"), "restart", None),
            Macro::new_string("System Info", Some("OpenShock"), "sysinfo", None),
            Macro::new_string("Echo Off", Some("OpenShock Setup"), "echo false", None),
            Macro::new_string(
                "Keep-Alive Off",
                Some("OpenShock Setup"),
                "keepalive false",
                None,
            ),
            Macro::new_bytes(
                "Setup Networks",
                Some("OpenShock Setup"),
                br#"networks "#.into(),
                None,
            ),
            Macro::new_string(
                "Setup Authtoken",
                Some("OpenShock Setup"),
                "authtoken ",
                None,
            ),
            Macro::new_string("Get Config (JSON)", Some("OpenShock"), "jsonconfig ", None),
            Macro::new_string("Get Config (Raw)", Some("OpenShock"), "rawconfig ", None),
            Macro::new_string(
                "CaiX Vib (ID 12345, 0.5s)",
                Some("OpenShock Setup"),
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":500}"#,
                None,
            ),
            Macro::new_string(
                "CaiX Vib (ID 12345, 1s)",
                Some("OpenShock Setup"),
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":1000}"#,
                None,
            ),
            Macro::new_bytes("Backspace", Some("OpenShock"), b"\x08".into(), None),
        ]);
        Self {
            // scrollbar_state: ScrollbarState::new(test_macros.len()),
            inner: test_macros,
            ui_state: MacrosPrompt::None,
            input: Input::default(),
            categories_selector: SingleLineSelectorState::new().with_selected(2),
            // categories: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
    pub fn len(&self) -> usize {
        self.category_filtered_macros().count()
    }
    fn selected_category(&self) -> MacroCategorySelection {
        let sub_index_amount = if self.has_no_category_macros() { 3 } else { 2 };
        match self.categories_selector.current_index {
            0 => MacroCategorySelection::AllBytes,
            1 => MacroCategorySelection::AllStrings,
            2 => MacroCategorySelection::AllMacros,
            3 if self.has_no_category_macros() => MacroCategorySelection::NoCategory,
            index => MacroCategorySelection::Category(
                self.categories()
                    .nth(index - sub_index_amount)
                    .unwrap_or(""),
            ),
        }
    }
    pub fn category_filtered_macros(&self) -> impl DoubleEndedIterator<Item = &Macro> {
        let category = self.selected_category();

        self.inner.iter().filter(move |m| match category {
            MacroCategorySelection::AllMacros => true,
            MacroCategorySelection::AllStrings => matches!(m.content, MacroContent::Text(_)),
            MacroCategorySelection::AllBytes => matches!(m.content, MacroContent::Bytes { .. }),
            MacroCategorySelection::NoCategory => m.category.is_none(),
            MacroCategorySelection::Category(cat) => m.category.as_deref() == Some(cat),
        })
    }
    pub fn as_table(&self) -> Table<'_> {
        let filtered = self
            .category_filtered_macros()
            .map(|m| m.title.as_str())
            .map(|m| Row::new(Text::raw(m)));

        let widths = [
            Constraint::Fill(1), // Constraint::Length(5)
        ];
        let table = Table::new(filtered, widths).row_highlight_style(Style::new().reversed());
        table
    }
    pub fn has_no_category_macros(&self) -> bool {
        self.inner.iter().any(|m| m.category.is_none())
    }
    pub fn categories<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a str> {
        let no_category = std::iter::once("No Category").filter(|_| self.has_no_category_macros());

        let categories: BTreeSet<&str> = self
            .inner
            .iter()
            .filter_map(|m| m.category.as_deref())
            .collect();

        no_category.chain(categories.into_iter())
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Macro {
    pub title: String,
    category: Option<String>,
    pub keybinding: Option<u8>,
    pub content: MacroContent,
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

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MacroContent {
    Empty,
    Text(String),
    Bytes { content: Vec<u8>, preview: String },
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
        keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
            category: category.map(|t| t.as_ref().into()),
            content: MacroContent::new_bytes(bytes),
            keybinding,
        }
    }
    pub fn new_string<T: AsRef<str>, S: AsRef<str>>(
        title: T,
        category: Option<T>,
        s: S,
        keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
            category: category.map(|t| t.as_ref().into()),
            content: MacroContent::Text(s.as_ref().into()),
            keybinding,
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
