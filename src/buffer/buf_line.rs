use std::borrow::Cow;

use bstr::ByteSlice;
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString, format_compact};
use ratatui::{
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use ratatui_macros::{line, span};
use tracing::debug;

use crate::{
    buffer::LineEnding,
    settings::Rendering,
    traits::{ByteSuffixCheck, FirstChars, LineHelpers},
};

#[derive(Debug, Clone)]
pub struct BufLine {
    pub(super) timestamp: DateTime<Local>,
    timestamp_str: CompactString,

    index_info: CompactString,

    pub(super) value: Line<'static>,

    /// How many vertical lines are needed in the terminal to fully show this line.
    // Truncated from usize, since even the ratatui sizes are capped there.
    rendered_line_height: u16,

    pub raw_buffer_index: usize,
    pub line_type: LineType,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LineType {
    Port {
        escaped_line_ending: Option<CompactString>,
    },
    User {
        is_bytes: bool,
        is_macro: bool,
        reloggable_raw: Vec<u8>,
    },
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
        line_ending: &LineEnding,
        now: DateTime<Local>,
        line_type: LineType,
    ) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        line.remove_unsavory_chars();

        // if !line.is_styled() && !line.is_empty() {
        //     assert!(line.spans.len() <= 1);
        //     determine_color(&mut line, &[]);
        // }

        let index_info = make_index_info(raw_value, raw_buffer_index, &line_type);

        let mut bufline = Self {
            timestamp_str: now.format(time_format).to_compact_string(),
            timestamp: now,
            index_info,
            value: line,
            raw_buffer_index,
            rendered_line_height: 0,
            line_type,
        };
        bufline.populate_line_ending(raw_value, line_ending);
        bufline.update_line_height(area_width, rendering);
        bufline
    }
    pub fn populate_line_ending(&mut self, full_line_slice: &[u8], line_ending: &LineEnding) {
        match &mut self.line_type {
            LineType::Port {
                escaped_line_ending,
            } => {
                if escaped_line_ending.is_some() {
                    unreachable!();
                }
                if full_line_slice.has_line_ending(line_ending) {
                    _ = escaped_line_ending
                        .insert(line_ending.as_bytes().escape_bytes().to_compact_string());
                }
            }
            // TODO?
            LineType::User { .. } => (),
        }
    }
    pub fn update_line(
        &mut self,
        line: Line<'static>,
        full_line_slice: &[u8],
        area_width: u16,
        rendering: &Rendering,
        line_ending: &LineEnding,
    ) {
        self.index_info = make_index_info(full_line_slice, self.raw_buffer_index, &self.line_type);

        self.value = line;
        self.value.remove_unsavory_chars();

        self.populate_line_ending(full_line_slice, line_ending);

        self.update_line_height(area_width, rendering);
    }

    pub fn update_line_height(&mut self, area_width: u16, rendering: &Rendering) -> usize {
        let para = Paragraph::new(self.as_line(rendering)).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_height = height as u16;
        height
    }

    pub fn get_line_height(&self) -> u16 {
        self.rendered_line_height
    }

    /// Returns an owned `Line` that borrows from the current line's spans.
    pub fn as_line(&self, rendering: &Rendering) -> Line {
        let borrowed_spans = self.value.borrowed_spans_iter();

        let indices_and_len = std::iter::once(Span::styled(
            Cow::Borrowed(self.index_info.as_ref()),
            Style::new().dark_gray(),
        ))
        .filter(|_| rendering.show_indices);

        let timestamp = std::iter::once(Span::styled(
            Cow::Borrowed(self.timestamp_str.as_ref()),
            Style::new().dark_gray(),
        ))
        .filter(|_| rendering.timestamps);

        let line_ending = std::iter::once(&self.line_type).filter_map(|lt| match lt {
            _ if !rendering.show_line_ending => None,
            LineType::Port {
                escaped_line_ending: Some(line_ending),
            } => Some(Span::styled(
                Cow::Borrowed(line_ending.as_str()),
                Style::new().dark_gray(),
            )),
            LineType::Port {
                escaped_line_ending: None,
            } => None,
            LineType::User { .. } => None,
        });

        let spans = timestamp
            .chain(indices_and_len)
            .chain(borrowed_spans)
            .chain(line_ending);

        Line::from_iter(spans)
    }

    pub fn index_in_buffer(&self) -> usize {
        self.raw_buffer_index
    }
}

fn make_index_info(
    full_line_slice: &[u8],
    start_index: usize,
    line_type: &LineType,
) -> CompactString {
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
