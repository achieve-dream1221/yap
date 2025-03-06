use std::{borrow::Cow, time::Instant};

use chrono::{DateTime, Local};
use ratatui::{
    layout::Size,
    style::{Style, Stylize},
    text::{Line, Span, ToSpan},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct Buffer {
    raw_buffer: Vec<u8>,
    pub lines: Vec<BufLine>,
    // This technically *works* but I have issues with it
    // Namely that this is the size of the terminal
    // and not the actual buffer render area.
    pub last_terminal_size: Size,
    // pub color_rules
}

#[derive(Debug)]
pub struct BufLine {
    value: String,
    rendered_line_count: usize,
    style: Option<Style>,
    // Might not be exactly accurate, but would be enough to place user input lines in proper space if needing to
    raw_buffer_index: usize,
    timestamp: String,
}
// Many changes needed, esp. in regards to current app-state things (index, width, color, showing timestamp)
impl BufLine {
    fn new(value: String, raw_buffer_index: usize, area_width: u16) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        let mut line = Self {
            value,
            raw_buffer_index,
            style: None,
            rendered_line_count: 0,
            timestamp: Local::now().format(time_format).to_string(),
        };
        line.update_line_count(area_width);
        line.determine_color();
        line
    }
    fn completed(&self, line_ending: &str) -> bool {
        self.value.ends_with(line_ending)
    }
    fn update_line_count(&mut self, area_width: u16) {
        let para = Paragraph::new(self.as_line()).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_count = height;
        // debug!("{self:?}");
    }
    fn determine_color(&mut self) {
        // Not sure if this is actually worth keeping, we'll see once I add proper custom rules.
        if self.style.is_some() {
            return;
        }
        // What do I pass into here?
        // The rules? Should it instead be an outside decider that supplies the color?

        if let Some(slice) = self.value.first_chars(5) {
            let mut style = Style::new();
            style = match slice {
                "USER>" => style.dark_gray(),
                "Got m" => style.blue(),
                "ID:0x" => style.green(),
                "Chan." => style.dark_gray(),
                "Mode:" => style.yellow(),
                "Power" => style.red(),
                _ => style,
            };

            if style != Style::new() {
                self.style = Some(style);
            }
        }
    }
    pub fn as_line(&self) -> Line {
        match self.style {
            Some(style) => Line::styled(&self.value, style),
            None => Line::raw(&self.value),
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            raw_buffer: Vec::with_capacity(1024),
            lines: Vec::with_capacity(1024),
            last_terminal_size: Size::default(),
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    // TODO also do append_user_bytes
    pub fn append_user_text(&mut self, text: &str) {
        // TODO dont use \n
        let value: String = format!("USER> {}\n", text.escape_debug());
        let line = BufLine::new(
            value,
            self.raw_buffer.len().saturating_sub(1),
            self.last_terminal_size.width,
        );
        self.lines.push(line);
    }

    // Forced to use Vec<u8> for now
    pub fn append_bytes(&mut self, bytes: &mut Vec<u8>) {
        let converted = String::from_utf8_lossy(&bytes).to_string();
        // TODO maybe do line ending splits at this level, so raw_buffer_index can be more accurate
        self.raw_buffer.append(bytes);

        let mut appending_to_last = self
            .lines
            .last()
            .map(|l| !l.completed(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]))
            .unwrap_or(false);
        // self.strings.iter_mut().for_each(|s| {
        // });

        // split_inclusive() or split()?
        for line in converted.split(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]) {
            // Removing messy-to-render characters, but they should be preserved in the raw_buffer for those who need to see them
            // TODO Replace tab with multiple spaces? (As \t causes smearing with ratatui currently.)
            // TODO Filter out ASCII control characters (like terminal bell)?
            let s = line.replace(&['\t', '\n', '\r'][..], "");
            // TODO UTF-8 multi byte preservation between \n's?
            // Since if I am getting only one byte per second or read, then `String::from_utf8_lossy` could fail extra for no reason.
            if appending_to_last {
                let line = self.lines.last_mut().expect("Promised line to append to");
                line.value.push_str(&s);
                line.determine_color();
                line.update_line_count(self.last_terminal_size.width);
                appending_to_last = false;
            } else {
                self.lines.push(BufLine::new(
                    s,
                    self.raw_buffer.len().saturating_sub(1),
                    self.last_terminal_size.width,
                ));
                // self.lines.push(Line::raw(line.to_owned()));
            }
        }
        // if let Some(line) = self.lines.last() {
        //     self.last_line_finished = line.ends_with(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]);
        // }

        // let _: Vec<_> = self
        //     .strings
        //     .iter()
        //     .map(|s| {
        //         debug!("{s:?}");
        //         s
        //     })
        //     .collect();
    }
    pub fn line_count(&self) -> usize {
        self.lines.iter().map(|l| l.rendered_line_count).sum()
    }
    pub fn update_line_count(&mut self) -> usize {
        self.lines.iter_mut().fold(0, |total, l| {
            l.update_line_count(self.last_terminal_size.width);

            total + l.rendered_line_count
        })
    }
    pub fn lines_iter(&self) -> impl Iterator<Item = Line> {
        // TODO styling based on line prefix
        self.lines.iter().map(|l| l.as_line())
        //     .map(|s| {
        //     if s.len() < 5 {
        //         Line::raw(s)
        //     } else {
        //         // TODO See if theres a more efficient matching method with variable-length patterns
        //         let slice = &s[..4];
        //         let line = Line::raw(s);
        //         match slice {
        //             "Got m" => line.blue(),
        //             "ID:0x" => line.green(),
        //             "Chan." => line.dark_gray(),
        //             "Mode:" => line.yellow(),
        //             "Power" => line.red(),
        //             _ => line,
        //         }
        //     }
        // })

        //     // std::iter::once(Line::raw(""))
    }
    pub fn terminal_paragraph(&self, buffer_wrapping: bool) -> Paragraph<'_> {
        // let lines: Vec<_> = self
        //     .buffer
        //     .lines
        //     .iter()
        //     .map(|s| Cow::Borrowed(s.as_str()))
        //     .map(|c| if styled { coloring(c) } else { Line::raw(c) })
        //     .collect();
        let lines: Vec<_> = self.lines_iter().collect();

        let para = Paragraph::new(lines).block(Block::new().borders(Borders::RIGHT));
        if buffer_wrapping {
            // TODO make better logic for this where it takes in the current scroll,
            // only rendering the lines intersecting with the buffer's "window",
            // and handling scrolling itself.
            para.wrap(Wrap { trim: false })
        } else {
            para
        }
    }
    pub fn clear(&mut self) {
        self.lines.clear();
        self.raw_buffer.clear();
    }
}

// pub fn colored_line<'a, L: Into<Line<'a>>>(text: L) -> Line<'a> {
//     if text.
//     let line = Line::from(text);

// }

trait FirstChars {
    fn first_chars(&self, char_count: usize) -> Option<&str>;
}

impl FirstChars for str {
    fn first_chars(&self, desired: usize) -> Option<&str> {
        let char_count = self.chars().count();
        if char_count < desired {
            None
        } else if char_count == desired {
            Some(self)
        } else {
            let end = self
                .char_indices()
                .nth(desired)
                .map(|(i, _)| i)
                .expect("Not enough chars?");
            Some(&self[..end])
        }
    }
}
