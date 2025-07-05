use std::thread::JoinHandle;

use camino::{Utf8Path, Utf8PathBuf};
use crossbeam::channel::Sender;
use ratatui::{
    prelude::*,
    widgets::{Cell, HighlightSpacing, Row, Table},
};

use serde::{Deserialize, Serialize};

use fs_err as fs;
#[cfg(feature = "defmt_watch")]
use takeable::Takeable;

use crate::app::Event;
#[cfg(feature = "defmt_watch")]
use crate::buffer::defmt::elf_watcher::ElfWatchHandle;

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

pub struct DefmtHelpers {
    pub recent_elfs: DefmtRecentElfs,
    #[cfg(feature = "defmt_watch")]
    pub watcher_handle: ElfWatchHandle,
    #[cfg(feature = "defmt_watch")]
    watcher_join_handle: Takeable<JoinHandle<()>>,
}

#[cfg(feature = "defmt_watch")]
impl Drop for DefmtHelpers {
    fn drop(&mut self) {
        use tracing::debug;
        use tracing::error;

        debug!("Shutting down file watcher");
        if self.watcher_handle.shutdown().is_ok() {
            let watcher = self.watcher_join_handle.take();
            if let Err(_) = watcher.join() {
                error!("File watcher thread closed with an error!");
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DefmtHelperBuildError {
    #[error(transparent)]
    RecentElfs(#[from] DefmtRecentError),

    #[cfg(feature = "defmt_watch")]
    #[error(transparent)]
    Watcher(#[from] notify::Error),
}

impl DefmtHelpers {
    pub fn build(
        #[cfg(feature = "defmt_watch")] event_tx: Sender<Event>,
    ) -> Result<Self, DefmtHelperBuildError> {
        #[cfg(feature = "defmt_watch")]
        {
            let (watcher_handle, watcher_join_handle) = ElfWatchHandle::build(event_tx)?;
            let watcher_join_handle = Takeable::new(watcher_join_handle);
            Ok(Self {
                recent_elfs: DefmtRecentElfs::load()?,
                watcher_handle,
                watcher_join_handle,
            })
        }

        #[cfg(not(feature = "defmt_watch"))]
        Ok(Self {
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

#[derive(Debug, thiserror::Error)]
pub enum DefmtRecentError {
    #[error("failed deserializing recent elfs: {0}")]
    Deser(#[from] toml::de::Error),
    #[error("failed serializing recent elfs: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error("failed recent elfs file op: {0}")]
    File(#[from] std::io::Error),
}

impl DefmtRecentElfs {
    pub fn load() -> Result<Self, DefmtRecentError> {
        let toml_path = Utf8PathBuf::from(DEFMT_RECENT_PATH);

        if toml_path.exists() {
            let recent_toml = fs::read_to_string(toml_path)?;

            toml::from_str(&recent_toml).map_err(Into::into)
        } else {
            fs::write(
                DEFMT_RECENT_PATH,
                toml::to_string(&DefmtRecentElfs::default())?.as_bytes(),
            )?;

            Ok(DefmtRecentElfs::default())
        }
    }
    pub fn elf_loaded(&mut self, newest: &Utf8Path) -> Result<(), DefmtRecentError> {
        let _ = self.last.insert(newest.to_owned());
        if let Some(found_index) = self.recent.iter().position(|p| *p == newest) {
            let element = self.recent.remove(found_index);
            self.recent.insert(0, element);
        } else {
            self.recent.insert(0, newest.to_owned());
        }

        self.recent.truncate(DEFMT_RECENT_MAX_AMOUNT);

        let recent_toml = toml::to_string(&self)?;

        fs::write(DEFMT_RECENT_PATH, recent_toml.as_bytes())?;

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

// pub fn defmt_buttons(decoder: &Option<DefmtDecoder>, frame: &mut Frame, screen: Rect) {
//     let decoder = decoder.as_ref();
// }
