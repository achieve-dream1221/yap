use ansi_to_tui::IntoText;
use chrono::{DateTime, Local};
use memchr::memmem::Finder;
use ratatui::{
    layout::Size,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget, Wrap,
    },
};
use ratatui_macros::{line, span};
use tracing::debug;

use crate::traits::{ByteSuffixCheck, RemoveUnsavory};

#[derive(Debug)]
pub struct BufLine {
    pub value: Line<'static>,
    // maybe? depends on whats easier to chain bytes from, for the hex view later
    // raw_value: Vec<u8>,
    /// How many vertical lines are needed in the terminal to fully show this line.
    rendered_line_height: usize,
    // Might not be exactly accurate, but would be enough to place user input lines in proper space if needing to
    raw_buffer_index: usize,
    timestamp: String,
}

// Many changes needed, esp. in regards to current app-state things (index, width, color, showing timestamp)
impl BufLine {
    pub fn new_with_line(
        mut line: Line<'static>,
        // raw_value: &[u8],
        raw_buffer_index: usize,
        area_width: u16,
        with_timestamp: bool,
    ) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        line.remove_unsavory_chars();

        let mut bufline = Self {
            value: line,
            // raw_value: raw_value.to_owned(),
            raw_buffer_index,
            // style: None,
            rendered_line_height: 0,
            timestamp: Local::now().format(time_format).to_string(),
        };
        bufline.update_line_height(area_width, with_timestamp);
        bufline
    }
    // pub fn new(
    //     raw_value: &[u8],
    //     raw_buffer_index: usize,
    //     area_width: u16,
    //     with_timestamp: bool,
    // ) -> Self {
    //     let time_format = "[%H:%M:%S%.3f] ";

    //     let value = determine_color(raw_value);

    //     let mut line = Self {
    //         value,
    //         // raw_value: raw_value.to_owned(),
    //         // raw_buffer_index,
    //         // style: None,
    //         rendered_line_height: 0,
    //         timestamp: Local::now().format(time_format).to_string(),
    //     };
    //     line.update_line_height(area_width, with_timestamp);
    //     line
    // }

    // pub fn new_user_line(raw)

    // fn completed(&self, line_ending: &str) -> bool {
    //     self.value.ends_with(line_ending)
    // }

    pub fn update_line_height(&mut self, area_width: u16, with_timestamp: bool) -> usize {
        let para = Paragraph::new(self.as_line(with_timestamp)).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_height = height;
        height
    }

    pub fn get_line_height(&self) -> usize {
        self.rendered_line_height
    }

    // pub fn append_bytes(&mut self, bytes: &[u8]) {
    //     self.raw_value.extend(bytes.iter());
    //     self.value = determine_color(&self.raw_value);
    // }

    pub fn as_line(&self, with_timestamp: bool) -> Line {
        let mut spans = self.value.clone().spans;

        if with_timestamp {
            spans.insert(0, span![Style::new().dark_gray(); &self.timestamp]);
        }
        spans.into()
        // match (self.style, with_timestamp) {
        //     (Some(style), true) => line![
        //         span![Style::new().dark_gray(); &self.timestamp],
        //         span![style; &self.value]
        //     ],
        //     (None, true) => line![
        //         ,
        //         &self.value
        //     ],

        //     (Some(style), false) => Line::styled(&self.value, style),
        //     (None, false) => Line::raw(&self.value),
        // }
    }

    pub fn index_in_buffer(&self) -> usize {
        self.raw_buffer_index
    }

    // pub fn bytes(&self) -> &[u8] {
    //     self.raw_value.as_slice()
    // }
}

fn determine_color(bytes: &[u8]) -> Line<'static> {
    let ratatui::text::Text {
        alignment: _alignment,
        style,
        mut lines,
    } = bytes.into_text().unwrap();
    debug!("{:?}", style);
    assert_eq!(lines.len(), 1);

    lines.pop().unwrap()

    // // Not sure if this is actually worth keeping, we'll see once I add proper custom rules.
    // if self.style.is_some() {
    //     return;
    // }
    // What do I pass into here?
    // The rules? Should it instead be an outside decider that supplies the color?

    // if let Some(slice) = self.value.first_chars(5) {
    //     let mut style = Style::new();
    //     style = match slice {
    //         "USER>" => style.dark_gray(),
    //         "Got m" => style.blue(),
    //         "ID:0x" => style.green(),
    //         "Chan." => style.dark_gray(),
    //         "Mode:" => style.yellow(),
    //         "Power" => style.red(),
    //         _ => style,
    //     };

    //     if style != Style::new() {
    //         self.style = Some(style);
    //     }
    // }
}
