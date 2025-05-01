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
    pub inner: BTreeSet<Macro>,
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

impl Macros {
    pub fn new() -> Self {
        // TODO Load from disk
        let test_macros = BTreeSet::from([
            Macro::new_string("Mrow!", "mrow", None),
            Macro::new_string("Version", "version", None),
            Macro::new_string("Factory Reset", "factoryreset", None),
            Macro::new_string("Restart", "restart", None),
            Macro::new_string("System Info", "sysinfo", None),
            Macro::new_string("Echo Off", "echo false", None),
            Macro::new_string("Keep-Alive Off", "keepalive false", None),
            Macro::new_string("Setup Networks", "networks ", None),
            Macro::new_string("Setup Authtoken", "authtoken ", None),
            Macro::new_string("Get Config (JSON)", "jsonconfig ", None),
            Macro::new_string("Get Config (Raw)", "rawconfig ", None),
            Macro::new_string(
                "CaiX Vib (ID 12345, 0.5s)",
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":500}"#,
                None,
            ),
            Macro::new_string(
                "CaiX Vib (ID 12345, 1s)",
                r#"rftransmit {"model":"caixianlin","id":12345,"type":"vibrate","intensity":5,"durationMs":1000}"#,
                None,
            ),
            Macro::new_bytes("Backspace", "\x08".as_bytes().into(), None),
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
        self.inner.len()
    }
    pub fn as_table(&self) -> Table<'_> {
        let rows = self
            .inner
            .iter()
            .map(|m| Row::new(vec![Cell::new(Text::raw(m.title.as_str()).centered())]));
        let widths = [
            Constraint::Fill(1), // Constraint::Length(5)
        ];
        let table = Table::new(rows, widths)
            .row_highlight_style(Style::new().reversed())
            // .highlight_spacing(HighlightSpacing::Always)
            // .highlight_symbol(">")
        ;
        table
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Macro {
    pub title: String,
    // category: String,
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
    pub fn new_bytes<T: AsRef<str>>(title: T, bytes: Vec<u8>, keybinding: Option<u8>) -> Self {
        Self {
            title: title.as_ref().into(),
            content: MacroContent::new_bytes(bytes),
            keybinding,
        }
    }
    pub fn new_string<T: AsRef<str>, S: AsRef<str>>(
        title: T,
        s: S,
        keybinding: Option<u8>,
    ) -> Self {
        Self {
            title: title.as_ref().into(),
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
