use ratatui::{style::Stylize, text::Line};
use tracing::debug;

use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct Buffer<'a> {
    raw_buffer: Vec<u8>,
    // Maybe convert to Vec<Lines>?
    // Should always be kept congruent with raw_buffer's contents
    // Need to consider how I'm going to include echos added to the buffer, if I ever need to rebuild string_buffer
    pub string: String,
    pub strings: Vec<String>,
    pub lines: Vec<Line<'a>>,
    // if not true, then the last line in [strings] is "incomplete" (no leading line-ending), and should be appended to
    last_line_finished: bool,
}

impl<'a> Buffer<'a> {
    pub fn new() -> Self {
        Self {
            raw_buffer: Vec::with_capacity(1024),
            string: String::new(),
            strings: Vec::new(),
            lines: Vec::new(),
            // there is no line to append to, so just act as if "finished"
            last_line_finished: true,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    // TODO also do append_user_bytes
    pub fn append_user_text(&mut self, text: &str) {
        self.last_line_finished = true;
        let input: String = format!("USER> {}", text.escape_debug());
        self.strings.push(input);
    }

    // Forced to use Vec<u8> for now
    pub fn append_bytes(&mut self, bytes: &mut Vec<u8>) {
        let converted = String::from_utf8_lossy(&bytes).to_string();
        self.raw_buffer.append(bytes);

        let mut appending = !self.last_line_finished;
        // self.strings.iter_mut().for_each(|s| {
        // });

        // split_inclusive() or split()?
        for line in converted.split(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]) {
            // Removing messy-to-render characters, but they should be preserved in the raw_buffer for those who need to see them
            // TODO Replace tab with multiple spaces? (As \t causes smearing with ratatui currently.)
            let s = line.replace(&['\t', '\n', '\r'][..], "");
            // TODO UTF-8 multi byte preservation between \n's?
            // Since if I am getting only one byte per second or read, then `String::from_utf8_lossy` could fail extra for no reason.
            if appending {
                self.strings
                    .last_mut()
                    .expect("Promised line to append to")
                    .push_str(&s);
                appending = false;
            } else {
                self.strings.push(s);
                // self.lines.push(Line::raw(line.to_owned()));
            }
        }
        if let Some(line) = self.strings.last() {
            self.last_line_finished = line.ends_with(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]);
        }

        // let _: Vec<_> = self
        //     .strings
        //     .iter()
        //     .map(|s| {
        //         debug!("{s:?}");
        //         s
        //     })
        //     .collect();
    }

    pub fn lines(&self) -> impl Iterator<Item = Line> {
        // TODO styling based on line prefix
        self.strings.iter().map(|s| {
            if s.len() < 5 {
                Line::raw(s)
            } else {
                // TODO See if theres a more efficient matching method with variable-length patterns
                let slice = &s[..4];
                let line = Line::raw(s);
                match slice {
                    "Got m" => line.blue(),
                    "ID:0x" => line.green(),
                    "Chan." => line.dark_gray(),
                    "Mode:" => line.yellow(),
                    "Power" => line.red(),
                    _ => line,
                }
            }
        })

        // std::iter::once(Line::raw(""))
    }
}

// pub fn colored_line<'a, L: Into<Line<'a>>>(text: L) -> Line<'a> {
//     if text.
//     let line = Line::from(text);

// }
