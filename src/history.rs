use arboard::Clipboard;
use itertools::Itertools;
use tracing::{debug, warn};
use tui_input::Input;

use crate::traits::LastIndex as _;

#[derive(Debug, Default)]
pub struct History {
    pub selected: Option<usize>,
    pub inner: Vec<String>,
}

pub struct UserInput {
    pub input_box: Input,
    pub all_text_selected: bool,
    pub preserved_input: Option<String>,
    pub search_result: Option<usize>,
    pub history: History,
    pub clipboard: Option<Clipboard>,
}

impl Default for UserInput {
    fn default() -> Self {
        let clipboard = Clipboard::new().map_or_else(
            |e| {
                warn!("Clipboard not supported? {e}");
                None
            },
            Some,
        );
        Self {
            input_box: Input::default(),
            all_text_selected: false,
            preserved_input: None,
            search_result: None,
            history: History::new(),
            clipboard,
        }
    }
}

impl UserInput {
    /// Clear the input box, preserves history.
    pub fn clear(&mut self) {
        self.input_box.reset();
        self.clear_history_selection();
        self.preserved_input = None;
        self.all_text_selected = false;
    }
    pub fn scroll_history(&mut self, up: bool) {
        if self.history.selected.is_none() && self.search_result.is_some() {
            self.history.selected = self.search_result;
        }

        let entry = self.history.scroll(up);
        // When first entering into history, cache the user's unsent input.
        if entry.is_some() && self.preserved_input.is_none() {
            self.preserved_input = Some(self.input_box.value().to_owned());
        }
        if let Some(entry) = entry {
            self.input_box = Input::new(entry.to_owned());
        } else {
            // Returning user's input text when exiting history
            if let Some(preserved) = self.preserved_input.take() {
                _ = self.search_result.take();
                self.input_box = preserved.into();
            }
        }
    }
    // TODO add way to get to bottom of history/back to preserved input without page up/down.
    pub fn find_input_in_history(&mut self) {
        // Skip if there's no text to search with.
        if self.input_box.value().is_empty()
            && self
                .preserved_input
                .as_ref()
                .map(String::is_empty)
                .unwrap_or(true)
        {
            assert!(self.search_result.is_none());
            return;
        }
        // Skip if there's no history to search in.
        if self.history.inner.is_empty() {
            return;
        }
        let history_len = self.history.inner.len();

        let find = |last: usize, query: &str| {
            self.history.inner[..last]
                .iter()
                .rev()
                .find_position(|h| h.starts_with(query))
                .map(|(i, h)| (last - i - 1, h))
        };

        let found = match (&self.search_result, &self.preserved_input) {
            (None, _) => {
                if self.input_box.value().is_empty() {
                    return;
                }
                find(history_len, self.input_box.value())
            }
            (Some(last_index), Some(saved_query)) => find(*last_index, saved_query.as_str()),
            (Some(_), None) => unreachable!(),
        };

        // debug!("found: {:?}", found);

        if let Some((new_index, result_text)) = found {
            if self.preserved_input.is_none() {
                self.preserved_input = Some(self.input_box.value().to_owned());
            }
            self.search_result = Some(new_index);
            self.input_box = result_text.as_str().into();
        }
    }
    pub fn clear_history_selection(&mut self) {
        self.history.clear_selection();
        self.preserved_input = None;
        self.search_result = None;
    }
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, entry: &str) {
        if entry.is_empty() {
            return;
        }
        // Checking if the given string exists at the end of our buffer
        if self.inner.last().map_or(false, |s| s.eq(entry)) {
            return;
        }
        // If it's instead further up the history, let's move it down to the bottom instead
        // TODO toggle for this behavior?
        if let Some(index) = self.inner.iter().position(|s| s.eq(entry)) {
            let existing = self.inner.remove(index);
            self.inner.push(existing);
        } else {
            // Doesn't exist, push an owned version.
            self.inner.push(entry.to_owned());
        }
    }
    pub fn get_selected(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.inner.get(index).map(String::as_str))
    }
    fn clear_selection(&mut self) {
        self.selected = None;
    }
    pub fn scroll(&mut self, up: bool) -> Option<&str> {
        if self.inner.is_empty() {
            return None;
        }

        if up {
            match self.selected {
                // At top of history, do nothing
                Some(0) => (),
                // Moving up the history (most recent elements first)
                Some(x) => self.selected = Some(x - 1),
                None => self.selected = Some(self.inner.last_index()),
            }
        } else {
            match self.selected {
                // Move down if there's elements to be expected
                Some(x) if x < self.inner.last_index() => self.selected = Some(x + 1),
                // No more elements, clear selection.
                Some(_) => self.clear_selection(),
                // Not in history, don't scroll.
                None => (),
            }
        }

        self.get_selected()
    }
}
