use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    f32::consts::PI,
    fmt,
    fs::FileType,
    path::Path,
};

use bstr::ByteVec;
use camino::Utf8PathBuf;
use compact_str::{CompactString, format_compact};
use crokey::KeyCombination;
use fs_err::{self as fs, DirEntry};
use indexmap::IndexMap;
use itertools::Either;
use ratatui::{
    layout::Constraint,
    style::{Style, Stylize},
    text::Text,
    widgets::{Cell, HighlightSpacing, Row, ScrollbarState, Table},
};
use tracing::{error, info};
use tui_input::Input;

use crate::{
    keybinds::Keybinds, traits::HasEscapedBytes, tui::single_line_selector::SingleLineSelectorState,
};

mod macro_nametag;
pub use macro_nametag::MacroNameTag;
mod tui;

// pub use macro_ref::MacroNameTag;
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
    pub all: BTreeMap<MacroNameTag, MacroContent>,

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

        let mut macros = Self {
            // scrollbar_state: ScrollbarState::new(test_macros.len()),
            all: BTreeMap::new(),
            // tx_queue: Vec::new(),
            // ui_state: MacrosPrompt::None,
            search_input: Input::default(),
            categories_selector: SingleLineSelectorState::new().with_selected(2),
            // categories: BTreeSet::new(),
        };
        macros.load_from_folder("../../example_macros").unwrap();

        macros
    }
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }
    pub fn visible_len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.filtered_macro_iter().count()
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
    fn category_filtered_macros(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&MacroNameTag, &MacroContent)> {
        let category = self.selected_category();

        self.all
            .iter()
            .filter(move |(tag, content)| match category {
                MacroCategorySelection::AllMacros => true,
                MacroCategorySelection::StringsOnly => !content.has_bytes,
                MacroCategorySelection::WithBytes => content.has_bytes,
                MacroCategorySelection::NoCategory => tag.category.is_none(),
                MacroCategorySelection::Category(cat) => tag.category.as_deref() == Some(cat),
            })
    }
    fn search_filtered_macros(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&MacroNameTag, &MacroContent)> {
        let query = self.search_input.value();
        let query_len = query.len();
        self.all.iter().filter(move |(tag, content)| {
            if tag.name.is_char_boundary(query_len) {
                tag.name[..query_len].eq_ignore_ascii_case(query)
            } else {
                false
            }
        })
    }
    pub fn filtered_macro_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&MacroNameTag, &MacroContent)> {
        if self.search_input.value().is_empty() {
            Either::Right(self.category_filtered_macros())
        } else {
            Either::Left(self.search_filtered_macros())
        }
    }
    pub fn as_table(&self, keybinds: &Keybinds, fuzzy_macro_name_match: bool) -> Table<'_> {
        let filtered = self
            .filtered_macro_iter()
            .map(|m| (m.0.name.as_str(), m))
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
    ) -> Result<Vec<&'a MacroNameTag>, Option<Vec<&'a MacroNameTag>>> {
        // ) -> Result<Vec<usize>, Option<Vec<&KeybindMacro>>> {
        let Some(v) = macro_keybinds.get(&key_combo) else {
            return Err(None);
        };

        let mut somes: Vec<&MacroNameTag> = Vec::new();
        let mut nones: Vec<&MacroNameTag> = Vec::new();

        v.iter().for_each(|config_tag| {
            let eq_result = self
                .all
                .iter()
                // .enumerate()
                .find(|(tag, content)| config_tag.eq(tag));
            match eq_result {
                Some((tag, content)) => {
                    somes.push(tag);
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
                .find(|(tag, content)| config_tag.eq_fuzzy(tag));
            match eq_result {
                Some((tag, content)) => somes.push(tag),
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
    pub fn load_from_folder<P: AsRef<Path>>(&mut self, folder: P) -> color_eyre::Result<()> {
        // TODO never return on error, just log and notify user to check logs for details.
        fn visit_dir(
            dir: &Path,
            new_macros: &mut BTreeMap<MacroNameTag, MacroContent>,
        ) -> color_eyre::Result<()> {
            for entry in fs::read_dir(dir)? {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        error!("Folder iteration error: {e}");
                        return Err(e.into());
                    }
                };

                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(e) => {
                        error!("Failed to get metadata for {}: {e}", entry.path().display());
                        return Err(e.into());
                    }
                };

                if metadata.is_dir() {
                    // Recurse into subdirectory
                    if let Err(e) = visit_dir(&entry.path(), new_macros) {
                        error!(
                            "Error traversing subdirectory {}: {e}",
                            entry.path().display()
                        );
                        // propagate error for now
                        return Err(e);
                    }
                    continue;
                }

                if !metadata.is_file() {
                    continue;
                }

                let file_name = Utf8PathBuf::from_path_buf(entry.path()).unwrap();
                if let Some(extension) = file_name.extension() {
                    if extension != "toml" {
                        continue;
                    }
                } else {
                    continue;
                }

                let mut file = match load_macros_from_path(&entry.path()) {
                    Ok(file) => file,
                    Err(e) => {
                        error!(
                            "Failed to read macros from file: {}. {e}",
                            entry.path().display()
                        );
                        continue;
                    }
                };

                if let Some(fallback_category) = &file.category_name {
                    file.macros
                        .iter_mut()
                        .filter(|m| m.category.is_none())
                        .for_each(|m| m.category = Some(fallback_category.to_owned()));
                } else {
                    file.macros
                        .iter_mut()
                        .filter(|m| m.category.is_none())
                        .for_each(|m| m.category = Some(file_name.file_stem().unwrap().into()));
                }

                for ser_macro in file.macros {
                    let (mut tag, content) = ser_macro.into_tag_and_content();

                    if let Some(category) = &mut tag.category {
                        if category.trim().is_empty() {
                            tag.category = None;
                        }
                    }

                    let old = new_macros.insert(tag, content);
                    if old.is_some() {
                        // TODO don't panic
                        panic!("Duplicate found!")
                    }
                }
            }
            Ok(())
        }

        let mut new_macros = BTreeMap::new();
        visit_dir(folder.as_ref(), &mut new_macros)?;
        self.all = new_macros;
        Ok(())
    }
}
#[derive(Debug, serde::Deserialize)]
struct MacroFile {
    #[serde(default)]
    #[serde(alias = "name")]
    #[serde(alias = "category")]
    category_name: Option<CompactString>,
    #[serde(rename = "macro")]
    macros: Vec<SerializedMacro>,
}
#[derive(Debug, serde::Deserialize)]
struct SerializedMacro {
    name: CompactString,
    category: Option<CompactString>,
    content: CompactString,
    line_ending: Option<CompactString>,
}
impl SerializedMacro {
    fn into_tag_and_content(self) -> (MacroNameTag, MacroContent) {
        let SerializedMacro {
            name,
            category,
            content,
            line_ending,
        } = self;

        (
            MacroNameTag { name, category },
            MacroContent::new_with_line_ending(&content, line_ending),
        )
    }
}
fn load_macros_from_path(path: &Path) -> color_eyre::Result<MacroFile> {
    let file = toml::from_str(fs_err::read_to_string(path)?.as_str())?;

    Ok(file)
}

#[derive(
    Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct MacroContent {
    pub content: CompactString,
    pub has_bytes: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escaped_line_ending: Option<CompactString>,
}

impl MacroContent {
    // pub fn new<S: AsRef<str>>(value: S) -> Self {
    //     Self {
    //         has_bytes: value.as_ref().has_escaped_bytes(),
    //         content: value.as_ref().into(),
    //         line_ending: None,
    //     }
    // }
    // pub fn update(&mut self, new: CompactString) {
    //     self.content = new;
    //     self.has_bytes = self.content.has_escaped_bytes();
    // }
    pub fn new_with_line_ending<S: AsRef<str>>(
        value: S,
        escaped_line_ending: Option<CompactString>,
    ) -> Self {
        Self {
            has_bytes: value.as_ref().has_escaped_bytes(),
            content: value.as_ref().into(),
            escaped_line_ending,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
    pub fn unescape_bytes(&self) -> Vec<u8> {
        Vec::unescape_bytes(&self.content)
    }
    pub fn as_str(&self) -> &str {
        &self.content
    }
}
