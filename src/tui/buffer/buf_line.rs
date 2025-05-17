use std::borrow::Cow;

use ansi_to_tui::IntoText;
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString, format_compact};
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

use crate::{
    settings::Rendering,
    traits::{ByteSuffixCheck, FirstChars, LineHelpers},
};

#[derive(Debug)]
pub struct BufLine {
    pub timestamp: DateTime<Local>,
    timestamp_str: CompactString,

    index_info: CompactString,

    value: Line<'static>,

    /// How many vertical lines are needed in the terminal to fully show this line.
    // Truncated from usize, since even the ratatui sizes are capped there.
    rendered_line_height: u16,

    pub(super) raw_buffer_index: usize,
    pub(super) line_type: LineType,
}

// impl PartialEq for BufLine {
//     fn eq(&self, other: &Self) -> bool {
//         self.timestamp == other.timestamp
//     }
// }

// impl Eq for BufLine {}

// impl PartialOrd for BufLine {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         self.timestamp.partial_cmp(&other.timestamp)
//     }
// }

// impl Ord for BufLine {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         self.timestamp.cmp(&other.timestamp)
//     }
// }

// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub(super) struct UserLine {
//     pub(super) is_bytes: bool,
//     pub(super) is_macro: bool,
// }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineType {
    Port,
    User { is_bytes: bool, is_macro: bool },
}

impl LineType {
    pub(super) fn is_bytes(&self) -> bool {
        match *self {
            LineType::User { is_bytes, .. } => is_bytes,
            _ => false,
        }
    }

    pub(super) fn is_macro(&self) -> bool {
        match *self {
            LineType::User { is_macro, .. } => is_macro,
            _ => false,
        }
    }
}

// Many changes needed, esp. in regards to current app-state things (index, width, color, showing timestamp)
impl BufLine {
    pub fn new_with_line(
        mut line: Line<'static>,
        raw_value: &[u8],
        raw_buffer_index: usize,
        area_width: u16,
        rendering: &Rendering,
        now: DateTime<Local>,
        line_type: LineType,
    ) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        line.remove_unsavory_chars();

        // if !line.is_styled() && !line.is_empty() {
        //     assert!(line.spans.len() <= 1);
        //     determine_color(&mut line, &[]);
        // }

        let index_info = index_info(raw_value, raw_buffer_index, line_type);

        let mut bufline = Self {
            timestamp_str: now.format(time_format).to_compact_string(),
            timestamp: now,
            index_info,
            value: line,
            raw_buffer_index,
            rendered_line_height: 0,
            line_type,
        };
        bufline.update_line_height(area_width, rendering);
        bufline
    }
    pub fn update_line(
        &mut self,
        mut line: Line<'static>,
        full_line_slice: &[u8],
        area_width: u16,
        rendering: &Rendering,
    ) {
        self.index_info = index_info(full_line_slice, self.raw_buffer_index, self.line_type);

        self.value = line;
        self.value.remove_unsavory_chars();
        self.update_line_height(area_width, rendering);
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

    pub fn update_line_height(&mut self, area_width: u16, rendering: &Rendering) -> usize {
        let para = Paragraph::new(self.as_line(rendering)).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_height = (height as u16);
        height
    }

    pub fn get_line_height(&self) -> u16 {
        self.rendered_line_height
    }

    /// Returns an owned `Line` that borrows from the current line's spans.
    pub fn as_line(&self, rendering: &Rendering) -> Line {
        let borrowed_spans = self.value.borrowed_spans_iter();

        let indices_span_iter = std::iter::once(Span::styled(
            Cow::Borrowed(self.index_info.as_ref()),
            Style::new().dark_gray(),
        ))
        .filter(|_| rendering.show_indices);

        let spans = std::iter::once(Span::styled(
            Cow::Borrowed(self.timestamp_str.as_ref()),
            Style::new().dark_gray(),
        ))
        .filter(|_| rendering.timestamps);

        let spans = spans.chain(indices_span_iter);

        let spans = spans.chain(borrowed_spans);

        Line::from_iter(spans)
    }

    pub fn index_in_buffer(&self) -> usize {
        self.raw_buffer_index
    }

    // pub fn timestamp(&self) -> (DateTime<Local>, &str) {
    //     (self.timestamp, &self.timestamp_str)
    // }

    // pub fn is_bytes(&self) -> bool {}

    // pub fn is_macro(&self) -> bool {}

    // pub fn bytes(&self) -> &[u8] {
    //     self.raw_value.as_slice()
    // }
}

// fn determine_color(line: &mut Line, rules: &[u8]) {
//     assert_eq!(line.spans.len(), 1);
//     if let Some(slice) = line.spans[0].content.first_chars(5) {
//         let mut style = Style::new();
//         style = match slice {
//             // "USER>" => style.dark_gray(),
//             "Got m" => style.blue(),
//             "ID:0x" => style.green(),
//             "Chan." => style.dark_gray(),
//             "Mode:" => style.yellow(),
//             "Power" => style.red(),
//             // "keepa" => style.red(),
//             _ => style,
//         };

//         if style != Style::new() {
//             line.style = style;
//             line.style_all_spans(style);
//         }
//     }
// }

fn index_info(full_line_slice: &[u8], start_index: usize, line_type: LineType) -> CompactString {
    if let LineType::User { .. } = line_type {
        format_compact!(
            "({start:06}->{end:06}, {len:3}) ",
            start = start_index,
            end = start_index + full_line_slice.len(),
            len = full_line_slice.len(),
        )
    } else {
        format_compact!(
            "({start:06}..{end:06}, {len:3}) ",
            start = start_index,
            end = start_index + full_line_slice.len(),
            len = full_line_slice.len(),
        )
    }
}

// fn determine_color(bytes: &[u8]) -> Line<'static> {
//     // let ratatui::text::Text {
//     //     alignment: _alignment,
//     //     style,
//     //     mut lines,
//     // } = bytes.into_line(Style::new(), None).unwrap();
//     // debug!("{:?}", style);
//     // assert_eq!(lines.len(), 1);

//     // lines.pop().unwrap()

//     bytes.into_line(None, Style::new()).unwrap()

//     // // Not sure if this is actually worth keeping, we'll see once I add proper custom rules.
//     // if self.style.is_some() {
//     //     return;
//     // }
//     // What do I pass into here?
//     // The rules? Should it instead be an outside decider that supplies the color?

//     // if let Some(slice) = self.value.first_chars(5) {
//     //     let mut style = Style::new();
//     //     style = match slice {
//     //         "USER>" => style.dark_gray(),
//     //         "Got m" => style.blue(),
//     //         "ID:0x" => style.green(),
//     //         "Chan." => style.dark_gray(),
//     //         "Mode:" => style.yellow(),
//     //         "Power" => style.red(),
//     //         _ => style,
//     //     };

//     //     if style != Style::new() {
//     //         self.style = Some(style);
//     //     }
//     // }
// }
