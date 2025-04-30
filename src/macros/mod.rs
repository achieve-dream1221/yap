use std::collections::BTreeSet;

use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    widgets::{HighlightSpacing, Row, Table},
};
use tui_input::Input;

use crate::tui::single_line_selector::SingleLineSelectorState;

pub struct Macros {
    pub macros: BTreeSet<Macro>,
    pub ui_state: MacrosPrompt,
    pub categories_selector: SingleLineSelectorState,
    pub input: Input,
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
        Self {
            macros: BTreeSet::from([
                Macro::new_string("Mrow!", "mrow", None),
                Macro::new_string("Get Version", "version", None),
                Macro::new_bytes("Backspace", "\x08".as_bytes().into(), None),
            ]),
            ui_state: MacrosPrompt::None,
            input: Input::default(),
            categories_selector: SingleLineSelectorState::new(),
            // categories: BTreeSet::new(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }
    pub fn len(&self) -> usize {
        self.macros.len()
    }
    pub fn as_table(&self) -> Table<'_> {
        let rows = self.macros.iter().map(|m| Row::new(vec![m.title.as_str()]));
        let widths = [Constraint::Fill(1), Constraint::Length(5)];
        let table = Table::new(rows, widths)
            .row_highlight_style(Style::new().reversed())
            // .highlight_spacing(HighlightSpacing::Always)
            // .highlight_symbol(">")
        ;
        table
    }
}

#[derive(Debug)]
pub struct Macro {
    title: String,
    // category: String,
    keybinding: Option<u8>,
    content: MacroContent,
}

// Custom Eq+Ord impls to avoid checking `content` when sorting.
impl PartialEq for Macro {
    fn eq(&self, other: &Self) -> bool {
        self.title == other.title && self.keybinding == other.keybinding
    }
}

impl Eq for Macro {}

impl PartialOrd for Macro {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.title.partial_cmp(&other.title) {
            Some(std::cmp::Ordering::Equal) => self.keybinding.partial_cmp(&other.keybinding),
            ord => ord,
        }
    }
}

impl Ord for Macro {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.title.cmp(&other.title) {
            std::cmp::Ordering::Equal => self.keybinding.cmp(&other.keybinding),
            ord => ord,
        }
    }
}

#[derive(Debug)]
pub enum MacroContent {
    Empty,
    Text(String),
    Bytes(Vec<u8>),
}

impl Macro {
    pub fn new_bytes<T: AsRef<str>>(title: T, bytes: Vec<u8>, keybinding: Option<u8>) -> Self {
        Self {
            title: title.as_ref().into(),
            content: MacroContent::Bytes(bytes),
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
            _ => "N/A",
        }
    }
}
