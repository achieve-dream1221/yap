// Current ELF:
// select an ELF
// select recent ELF
// ---
// SETTINGS

use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};
use ratatui::{
    prelude::*,
    widgets::{Cell, HighlightSpacing, Row, Table},
};
use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};

use fs_err as fs;

use crate::buffer::defmt::DefmtDecoder;

const DEFMT_RECENT_PATH: &str = "yap_defmt_recent.toml";

const DEFMT_RECENT_MAX_AMOUNT: usize = 10;

#[derive(Debug, PartialEq)]
pub enum DefmtPopupSelection {
    SelectElf,
    RecentElfs,
    Settings(usize),
}

impl From<usize> for DefmtPopupSelection {
    fn from(value: usize) -> Self {
        match value {
            0 => Self::SelectElf,
            1 => Self::RecentElfs,
            x => Self::Settings(x - 2),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DefmtMeow {
    // #[serde(skip)]
    // pub file_explorer: Option<FileExplorer>,
    pub recent_elfs: DefmtRecentElfs,
}

impl DefmtMeow {
    pub fn load() -> Result<Self, toml::de::Error> {
        Ok(Self {
            // file_explorer: None,
            recent_elfs: DefmtRecentElfs::load()?,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DefmtRecentElfs {
    #[serde(default)]
    last: Option<Utf8PathBuf>,
    #[serde(default)]
    recent: Vec<Utf8PathBuf>,
}

impl DefmtRecentElfs {
    pub fn load() -> Result<Self, toml::de::Error> {
        let toml_path = Utf8PathBuf::from(DEFMT_RECENT_PATH);

        if toml_path.exists() {
            let recent_toml = fs::read_to_string(toml_path).unwrap();

            toml::from_str(&recent_toml)
        } else {
            fs::write(
                DEFMT_RECENT_PATH,
                toml::to_string(&DefmtRecentElfs::default())
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();

            Ok(DefmtRecentElfs::default())
        }
    }
    pub fn elf_loaded(&mut self, newest: &Utf8Path) -> Result<(), toml::ser::Error> {
        let _ = self.last.insert(newest.to_owned());
        if let Some(found_index) = self.recent.iter().position(|p| *p == newest) {
            let element = self.recent.remove(found_index);
            self.recent.insert(0, element);
        } else {
            self.recent.insert(0, newest.to_owned());
        }

        self.recent.truncate(DEFMT_RECENT_MAX_AMOUNT);

        let recent_toml = toml::to_string(&self).unwrap();

        fs::write(DEFMT_RECENT_PATH, recent_toml.as_bytes()).unwrap();

        Ok(())
    }
    pub fn as_table(&self) -> Table<'static> {
        let paths_iter = self.recent.iter().map(|p| {
            let exists = p.exists();
            (p, exists)
        });

        let mut rows: Vec<Row<'static>> = Vec::new();
        let constraints = [Constraint::Fill(1)];

        for (path, exists) in paths_iter {
            let missing_suffix = if exists { "" } else { " [?]" };
            let row_style = if exists {
                Style::new()
            } else {
                Style::new().yellow()
            };
            let row_text = format!("{path}{missing_suffix}");
            let row = Row::new(vec![Cell::new(row_text)]).style(row_style);
            rows.push(row);
        }

        Table::new(rows, constraints)
            .row_highlight_style(Style::new().reversed())
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_symbol(">>")
    }
    pub fn nth_path(&self, nth: usize) -> Option<&Utf8Path> {
        self.recent.get(nth).map(Utf8PathBuf::as_path)
    }
    pub fn last(&self) -> Option<&Utf8Path> {
        self.last.as_ref().map(Utf8PathBuf::as_path)
    }
    pub fn recents_len(&self) -> usize {
        self.recent.len()
    }
}

pub fn defmt_buttons(decoder: &Option<DefmtDecoder>, frame: &mut Frame, screen: Rect) {
    let decoder = decoder.as_ref();
}
