use arboard::Clipboard;
use tui_input::Input;

#[derive(Debug, Default)]
pub struct History {
    pub selected: Option<usize>,
    pub history: Vec<String>,
}

pub struct UserInput {
    pub input_box: Input,
    pub preserved_input: Option<String>,
    pub history: History,
    pub clipboard: Clipboard,
}

impl Default for UserInput {
    fn default() -> Self {
        Self {
            input_box: Input::default(),
            preserved_input: None,
            history: History::new(),
            clipboard: Clipboard::new().unwrap(),
        }
    }
}

impl UserInput {
    pub fn reset(&mut self) {
        self.input_box.reset();
        self.history.clear_selection();
        self.preserved_input = None;
    }
    pub fn scroll(&mut self, up: bool) {
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
                self.input_box = preserved.into();
            }
        }
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
        if self.history.last().map_or(false, |s| s.eq(entry)) {
            return;
        }
        self.history.push(entry.to_owned());
        self.clear_selection();
    }
    pub fn get_selected(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.history.get(index).map(String::as_str))
    }
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }
    pub fn scroll(&mut self, up: bool) -> Option<&str> {
        if self.history.is_empty() {
            return None;
        }

        if up {
            match self.selected {
                // At top of history, do nothing
                Some(0) => (),
                // Moving up the history (most recent elements first)
                Some(x) => self.selected = Some(x - 1),
                None => self.selected = Some(self.history.len() - 1),
            }
        } else {
            match self.selected {
                // Move down if there's elements to be expected
                Some(x) if x < self.history.len() - 1 => self.selected = Some(x + 1),
                // No more elements, clear selection.
                Some(_) => self.clear_selection(),
                // Not in history, don't scroll.
                None => (),
            }
        }

        self.get_selected()
    }
}
