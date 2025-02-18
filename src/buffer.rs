use ratatui::text::Line;

use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct Buffer<'a> {
    raw_buffer: Vec<u8>,
    // Maybe convert to Vec<Lines>?
    // Should always be kept congruent with raw_buffer's contents
    // Need to consider how I'm going to include echos added to the buffer, if I ever need to rebuild string_buffer
    strings: Vec<String>,
    lines: Vec<Line<'a>>,
    last_line_finished: bool,
}

impl<'a> Buffer<'a> {
    pub fn new() -> Self {
        Self {
            raw_buffer: Vec::with_capacity(1024),
            strings: Vec::new(),
            lines: Vec::new(),
            last_line_finished: true,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    // Forced to use Vec<u8> for now
    pub fn append_bytes(&mut self, bytes: &mut Vec<u8>) {
        let converted = String::from_utf8_lossy(&bytes).to_string();
        self.raw_buffer.append(bytes);

        let mut appending = !self.last_line_finished;
        for line in converted.split_inclusive(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]) {
            if appending {
                // Unwrap should be safe due to above check
                self.strings.last_mut().unwrap().push_str(line);
                appending = false;
            } else {
                self.strings.push(line.to_owned());
            }
        }
        if let Some(line) = self.strings.last() {
            self.last_line_finished = line.ends_with(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]);
        }
    }

    pub fn lines(&self) -> impl Iterator<Item = Line> {
        // TODO styling based on line prefix
        self.strings.iter().map(|s| Line::raw(s))

        // std::iter::once(Line::raw(""))
    }
}
