use std::{cmp::Ordering, thread::JoinHandle};

use ansi_to_tui::{IntoText, LossyFlavor};
use bstr::{ByteSlice, ByteVec};
use buf_line::{BufLine, LineType};
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString};
use itertools::{Either, Itertools};
use logging::LoggingHandle;
use memchr::memmem::Finder;
use ratatui::{
    layout::{Rect, Size},
    style::{Color, Style, Stylize, palette::material::PINK},
    symbols,
    text::{Line, Span},
    widgets::ScrollbarState,
};
use ratatui_macros::span;
use takeable::Takeable;
use tracing::{debug, error, info, warn};

use crate::{
    changed,
    errors::YapResult,
    settings::{Logging, LoggingType, Rendering},
    traits::{ByteSuffixCheck, LineHelpers, interleave_by},
    tui::color_rules::ColorRules,
};

mod buf_line;
mod hex_spans;
mod logging;
pub use logging::DEFAULT_TIMESTAMP_FORMAT;
mod tui;

#[cfg(test)]
mod tests;
// mod wrap;

// use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

#[derive(Debug)]
pub struct BufferState {
    vert_scroll: usize,
    scrollbar_state: ScrollbarState,
    stuck_to_bottom: bool,
    hex_bytes_per_line: u8,
    hex_section_width: u16,
}

impl UserEcho {
    /// Determines whether a given `BufLine` should be displayed based on the current `UserEcho` setting.
    ///
    /// Returns `true` if the line passes the filter and should be shown, or `false` otherwise.
    ///
    /// Filtering rules:
    /// - `None`: Do not display any user lines.
    /// - `All`: Display all user lines.
    /// - `NoBytes`: Display all user lines except those marked as bytes (if they have any unprintable bytes when escaped).
    /// - `NoMacros`: Display all user lines except those marked as macros.
    /// - `NoMacrosOrBytes`: Display only user lines that are neither bytes nor macros.
    fn filter_user_line(&self, buf_line: &BufLine) -> bool {
        match self {
            UserEcho::None => false,
            UserEcho::All => true,
            UserEcho::NoBytes => !buf_line.line_type.is_bytes(),
            UserEcho::NoMacros => !buf_line.line_type.is_macro(),
            UserEcho::NoMacrosOrBytes => {
                !buf_line.line_type.is_bytes() && !buf_line.line_type.is_macro()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum LineEnding {
    None,
    Byte(u8),
    MultiByte(Finder<'static>),
}

impl PartialEq<str> for LineEnding {
    fn eq(&self, other: &str) -> bool {
        let other_esc = Vec::unescape_bytes(other);
        match self {
            LineEnding::None => other.is_empty(),
            _ if other.is_empty() => false,
            LineEnding::Byte(_) if other_esc.len() != 1 => false,
            LineEnding::Byte(byte) => *byte == other_esc[0],
            LineEnding::MultiByte(finder) => finder.needle() == other_esc,
        }
    }
}

impl PartialEq<&str> for LineEnding {
    fn eq(&self, other: &&str) -> bool {
        self.eq(*other)
    }
}

impl PartialEq<LineEnding> for &str {
    fn eq(&self, other: &LineEnding) -> bool {
        other.eq(*self)
    }
}

impl PartialEq<LineEnding> for str {
    fn eq(&self, other: &LineEnding) -> bool {
        other.eq(self)
    }
}

impl PartialEq<[u8]> for LineEnding {
    fn eq(&self, other: &[u8]) -> bool {
        match self {
            LineEnding::None => other.is_empty(),
            _ if other.is_empty() => false,
            LineEnding::Byte(_) if other.len() != 1 => false,
            LineEnding::Byte(byte) => *byte == other[0],
            LineEnding::MultiByte(finder) => finder.needle() == other,
        }
    }
}

impl PartialEq<&[u8]> for LineEnding {
    fn eq(&self, other: &&[u8]) -> bool {
        self.eq(*other)
    }
}

// impl AsRef<[u8]> for LineEnding {
//     fn as_ref(&self) -> &[u8] {
//         match self {
//             LineEnding::None => &[],
//             LineEnding::Byte(cs) => cs.as_bytes(),
//             LineEnding::MultiByte(cs, _) => cs.as_bytes(),
//         }
//     }
// }

// impl AsRef<str> for LineEnding {
//     fn as_ref(&self) -> &str {
//         match self {
//             LineEnding::None => "",
//             LineEnding::Byte(cs) => cs.as_str(),
//             LineEnding::MultiByte(cs, _) => cs.as_str(),
//         }
//     }
// }

impl From<&str> for LineEnding {
    fn from(value: &str) -> Self {
        if value.is_empty() {
            LineEnding::None
        } else {
            let unescaped = Vec::unescape_bytes(value);
            debug_assert!(!unescaped.is_empty());
            if value.len() == 1 {
                LineEnding::Byte(unescaped[0])
            } else {
                let finder = Finder::new(&unescaped).into_owned();
                LineEnding::MultiByte(finder)
            }
        }
    }
}

impl From<&[u8]> for LineEnding {
    fn from(value: &[u8]) -> Self {
        if value.is_empty() {
            LineEnding::None
        } else {
            if value.len() == 1 {
                LineEnding::Byte(value[0])
            } else {
                let finder = Finder::new(value).into_owned();
                LineEnding::MultiByte(finder)
            }
        }
    }
}

// TODO tests for buffer behavior with new lines
// (i broke it once before with tests passing, so, bleh)

struct RawBuffer {
    inner: Vec<u8>,
    /// Time-tagged indexes into `raw_buffer`, from each input from the port.
    buffer_timestamps: Vec<(usize, DateTime<Local>)>,
}

struct StyledLines {
    rx: Vec<BufLine>,
    last_rx_completed: bool,
    tx: Vec<BufLine>,
}

pub struct Buffer {
    raw: RawBuffer,
    styled_lines: StyledLines,

    /// The last-known size of the area given to render the buffer in
    last_terminal_size: Size,

    pub state: BufferState,

    rendering: Rendering,

    line_ending: LineEnding,

    color_rules: ColorRules,

    pub log_handle: LoggingHandle,
    log_thread: Takeable<JoinHandle<()>>,
    log_settings: Logging,
}

impl Drop for Buffer {
    fn drop(&mut self) {
        debug!("Shutting down Logging worker");
        if self.log_handle.shutdown().is_ok() {
            let log_thread = self.log_thread.take();

            if let Err(_) = log_thread.join() {
                error!("Logging thread closed with an error!");
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    strum::VariantArray,
)]
#[strum(serialize_all = "title_case")]
pub enum UserEcho {
    #[strum(serialize = "false")]
    None,
    #[strum(serialize = "true")]
    All,
    // #[strum(serialize = "All but No Macros")]
    NoMacros,
    // #[strum(serialize = "All but No Bytes")]
    NoBytes,
    NoMacrosOrBytes,
}

// impl Default for Buffer {
//     fn default() -> Self {
//         Self::new()
//     }
// }

impl Buffer {
    // TODO lower sources of truth for all this.
    // Rc<Something> with the settings that's shared around?
    pub fn new(line_ending: &[u8], rendering: Rendering, logging: Logging) -> Self {
        let line_ending: LineEnding = line_ending.into();
        let (log_handle, log_thread) = LoggingHandle::new(line_ending.clone(), logging.clone());

        Self {
            raw: RawBuffer {
                inner: Vec::with_capacity(1024),

                buffer_timestamps: Vec::with_capacity(1024),
            },
            styled_lines: StyledLines {
                rx: Vec::with_capacity(1024),
                last_rx_completed: true,
                tx: Vec::with_capacity(1024),
            },
            last_terminal_size: Size::default(),
            state: BufferState {
                vert_scroll: 0,
                scrollbar_state: ScrollbarState::default(),
                stuck_to_bottom: false,
                hex_bytes_per_line: 0,
                hex_section_width: 0,
            },
            rendering,
            line_ending,
            color_rules: ColorRules::load_from_file("../../color_rules.toml"),
            log_handle,
            log_thread: Takeable::new(log_thread),
            log_settings: logging,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    pub fn append_user_bytes(&mut self, bytes: &[u8], line_ending: &[u8], is_macro: bool) {
        let now = Local::now();
        let text: Span = bytes
            .iter()
            .chain(line_ending.iter())
            .map(|b| format!("\\x{:02X}", b))
            .join("")
            .into();
        let text = text.dark_gray().italic().bold();

        let user_span = span!(Color::DarkGray; "BYTE> ");

        let line = Line::from(vec![user_span, text]);

        // line.spans.insert(0, user_span.clone());
        // line.style_all_spans(Color::DarkGray.into());
        let user_buf_line = BufLine::new_with_line(
            line,
            bytes,
            self.raw.inner.len(), // .max(1)
            self.last_terminal_size.width,
            &self.rendering,
            now,
            LineType::User {
                is_bytes: true,
                is_macro,
            },
        );

        self.styled_lines.last_rx_completed = self
            .rendering
            .echo_user_input
            .filter_user_line(&user_buf_line)
            || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));
        if self.log_handle.logging_active() {
            match self.log_settings.log_file_type {
                LoggingType::Binary => (),
                LoggingType::Text | LoggingType::Both => self
                    .log_handle
                    .log_tx_bytes(now, bytes.to_owned(), line_ending.to_owned())
                    .unwrap(),
            }
        }
        self.styled_lines.tx.push(user_buf_line);
    }

    pub fn append_user_text(&mut self, text: &str, line_ending: &[u8], is_macro: bool) {
        let now = Local::now();
        let escaped_line_ending = line_ending.escape_bytes().to_string();
        let escaped_chained: Vec<u8> = text
            .as_bytes()
            .iter()
            .chain(escaped_line_ending.as_bytes().iter())
            .map(|i| *i)
            .collect();

        let user_span = span!(Color::DarkGray;"USER> ");
        // let Text { lines, .. } = text;
        // TODO HANDLE MULTI-LINE USER INPUT AAAA
        for (trunc, orig, _indices) in line_ending_iter(&escaped_chained, &self.line_ending) {
            // not sure if i want to ansi-style user text?
            // let mut line = match trunc.into_line_lossy(Style::new()) {
            //     Ok(line) => line,
            //     Err(_) => {
            //         error!("ansi-to-tui failed to parse input! Using unstyled text.");
            //         Line::from(String::from_utf8_lossy(trunc).to_string())
            //     }
            // };

            let mut line = Line::from(String::from_utf8_lossy(trunc).to_string());

            line.spans.insert(0, user_span.clone());
            line.style_all_spans(Color::DarkGray.into());
            let user_buf_line = BufLine::new_with_line(
                line,
                orig,
                self.raw.inner.len(), // .max(1)
                self.last_terminal_size.width,
                &self.rendering,
                now,
                LineType::User {
                    is_bytes: false,
                    is_macro,
                },
            );
            // Used to be out of the for loop.
            self.styled_lines.last_rx_completed = self
                .rendering
                .echo_user_input
                .filter_user_line(&user_buf_line)
                || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));
            if self.log_handle.logging_active() {
                match self.log_settings.log_file_type {
                    LoggingType::Binary => (),
                    LoggingType::Text | LoggingType::Both => self
                        .log_handle
                        .log_tx_bytes(now, text.as_bytes().to_owned(), line_ending.to_owned())
                        .unwrap(),
                }
            }
            self.styled_lines.tx.push(user_buf_line);
        }
    }

    /// Consumes **post**-split by line endings slices,
    /// either creating a new line or appending to the last one.
    fn consume_port_bytes<'a>(
        &mut self,
        trunc: &'a [u8],
        orig: &'a [u8],
        start_index: usize,
        known_time: Option<DateTime<Local>>,
    ) {
        // debug!("{trunc:?}, {orig:?}");
        assert!(
            trunc.len() <= orig.len(),
            "truncated buffer can't be larger than original"
        );
        let append_to_last = !self.styled_lines.last_rx_completed;
        if orig.is_empty() {
            return;
        }

        if append_to_last {
            let last_line = self
                .styled_lines
                .rx
                .last_mut()
                .expect("can't append to nothing");
            let last_index = last_line.index_in_buffer();

            let slice = &self.raw.inner[last_index..start_index + trunc.len()];
            // info!("AAAFG: {:?}", slice);
            let lossy_flavor = if self.rendering.escape_invalid_bytes {
                LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
            } else {
                LossyFlavor::replacement_char()
            };
            let mut line = match slice.into_line_lossy(Style::new(), lossy_flavor) {
                Ok(line) => line,
                Err(_) => {
                    error!("ansi-to-tui failed to parse input! Using unstyled text.");
                    Line::from(String::from_utf8_lossy(slice).to_string())
                }
            };
            // debug!(
            //     "buf_index: {last_index}, update: {line}",
            //     line = line
            //         .spans
            //         .iter()
            //         .map(|s| s.content.as_ref())
            //         .join("")
            //         .escape_default()
            // );

            // if line.width() >= 5 {
            //     line.style_slice(1..3, Style::new().red().italic());
            // }

            let line_opt = self.color_rules.apply_onto(slice, line);

            if let Some(line) = line_opt {
                last_line.update_line(line, slice, self.last_terminal_size.width, &self.rendering);
            } else {
                _ = self.styled_lines.rx.pop();
                self.styled_lines.last_rx_completed = true;
                // last_line.clear_line();
            }
        } else {
            let lossy_flavor = if self.rendering.escape_invalid_bytes {
                LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
            } else {
                LossyFlavor::replacement_char()
            };
            let mut line = match trunc.into_line_lossy(Style::new(), lossy_flavor) {
                Ok(line) => line,
                Err(_) => {
                    error!("ansi-to-tui failed to parse input! Using unstyled text.");
                    Line::from(String::from_utf8_lossy(trunc).to_string())
                }
            };

            let line_opt = self.color_rules.apply_onto(trunc, line);

            // if line.width() >= 5 {
            //     line.style_slice(1..3, Style::new().red().italic());
            // }

            // if !line.is_styled() {
            //     assert!(line.spans.len() <= 1);
            // }

            // debug!(
            //     "buf_index: {start_index}, new: {line}",
            //     line = line
            //         .spans
            //         .iter()
            //         .map(|s| s.content.as_ref())
            //         .join("")
            //         .escape_default()
            // );

            // let line = line_opt.unwrap_or_default();
            if let Some(line) = line_opt {
                self.styled_lines.rx.push(BufLine::new_with_line(
                    line,
                    orig,
                    start_index,
                    self.last_terminal_size.width,
                    &self.rendering,
                    known_time.unwrap_or_else(Local::now),
                    LineType::Port,
                ));
            }
        };
        let last_rx_completed = self.raw.inner.has_line_ending(&self.line_ending);
        self.styled_lines.last_rx_completed = last_rx_completed;
    }

    // Forced to use Vec<u8> for now
    pub fn append_rx_bytes(&mut self, bytes: &mut Vec<u8>) {
        let now = Local::now();
        let mut index = self.raw.inner.len();
        self.raw.buffer_timestamps.push((index, now));
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);
        self.log_handle.log_rx_bytes(now, bytes.clone()).unwrap();
        for (trunc, orig, indices) in line_ending_iter(bytes, &self.line_ending.clone()) {
            index = self.raw.inner.len();
            self.raw.inner.extend(orig);
            self.consume_port_bytes(trunc, orig, index, Some(now));
        }
    }

    /// Clears `self.lines` and reiterates through the whole `raw_buffer` again.
    ///
    /// Avoid running when possible, isn't cheap to run.
    pub fn reconsume_raw_buffer(&mut self) {
        if self.raw.inner.is_empty() {
            warn!("Can't reconsume an empty buffer!");
            return;
        }

        // let _ = std::mem::take(&mut self.lines);
        self.styled_lines.rx.clear();

        self.log_handle.clear_current_logs().unwrap();

        // Taking these variables out of `self` temporarily to allow running &mut self methods while holding
        // references to these.
        let timestamps = std::mem::take(&mut self.raw.buffer_timestamps);
        let user_lines = std::mem::take(&mut self.styled_lines.tx);
        let orig_buf_len = self.raw.inner.len();
        let buffer = std::mem::replace(&mut self.raw.inner, Vec::with_capacity(orig_buf_len));
        let line_ending = self.line_ending.clone();
        let user_echo = self.rendering.echo_user_input.clone();

        // No lines to append to.
        self.styled_lines.last_rx_completed = true;

        // Getting all time-tagged indices in the buffer where either
        // 1. Data came in through the port
        // 2. The user sent data
        let interleaved_points = interleave_by(
            timestamps
                .iter()
                .map(|(index, timestamp)| (*index, *timestamp, false))
                // Add a "finale" element to capture any remaining buffer, always placed at the end.
                .chain(std::iter::once((orig_buf_len, Local::now(), false))),
            user_lines
                .iter()
                // If a user line isn't visible, ignore it when taking external new-lines into account.
                .filter(|b| user_echo.filter_user_line(b))
                .map(|b| (b.raw_buffer_index, b.timestamp, true)),
            // Interleaving by sorting in order of raw_buffer_index, if they're equal, then whichever has a sooner timestamp.
            |port, user| match port.0.cmp(&user.0) {
                Ordering::Equal => port.1 <= user.1,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        );

        let mut new_index = 0;
        debug!("total len: {orig_buf_len}");

        let buffer_slices = interleaved_points
            .tuple_windows()
            // Filtering out some empty slices, unless they indicate a user event.
            .filter(|((start_index, _, was_user_line), (end_index, _, _))| {
                start_index != end_index || *was_user_line
            })
            // Building the parent slices (pre-newline splitting)
            .map(
                |((start_index, timestamp, was_user_line), (end_index, _, _))| {
                    (
                        &buffer[start_index..end_index],
                        timestamp,
                        was_user_line,
                        (start_index, end_index),
                    )
                },
            );

        // let buffer_slices: Vec<_> = buffer_slices.collect();
        // debug!("{buffer_slices:#?}");

        // info!("Slicing raw buffer!");
        for (slice, timestamp, was_user_line, (slice_start, slice_end)) in buffer_slices {
            // debug!("{slice:#?}");
            // If this was where a user line we allow to render is,
            // then we'll finish this line early if it's not already finished.
            if was_user_line {
                self.styled_lines.last_rx_completed = true;
                continue;
            }

            // info!(
            //     "Getting {le} slices from [{slice_start}..{slice_end}], {timestamp}, {was_user_line}",
            //     le = line_ending.escape_debug()
            // );
            for (trunc, orig, (orig_start, orig_end)) in line_ending_iter(slice, &line_ending) {
                // info!(
                //     "trunc: {trunc_len}, orig: {orig_len}. [{start}..{end}]",
                //     trunc_len = trunc.len(),
                //     orig_len = orig.len(),
                //     start = orig_start + slice_start,
                //     end = orig_end + slice_start,
                // );
                self.raw.inner.extend(orig);
                self.consume_port_bytes(trunc, orig, new_index, Some(timestamp));
                new_index += orig.len();
            }
        }

        // Asserting that our work seems correct.
        assert_eq!(
            orig_buf_len,
            self.raw.inner.len(),
            "Buffer size should not have changed during reconsumption."
        );
        assert_eq!(
            new_index, orig_buf_len,
            "Iterator's slices should have same total length as raw buffer."
        );
        self.styled_lines.rx.windows(2).for_each(|lines| {
            assert!(
                lines[0].raw_buffer_index < lines[1].raw_buffer_index,
                "Port lines should be in exact ascending order by index."
            )
        });

        // Returning variables we stole back to self.
        // (excluding the raw buffer since that got reconsumed gradually back into self)
        self.raw.buffer_timestamps = timestamps;
        self.styled_lines.tx = user_lines;
        self.scroll_by(0);
    }

    // pub fn update_line_ending(&mut self, line_ending: &str) {
    pub fn update_line_ending(&mut self, line_ending: &[u8]) {
        if self.line_ending != line_ending {
            self.line_ending = line_ending.into();
            self.reconsume_raw_buffer();
        }
    }
    pub fn update_render_settings(&mut self, rendering: Rendering) {
        let old = std::mem::replace(&mut self.rendering, rendering);
        let new = &self.rendering;
        let should_reconsume =
            changed!(old, new, echo_user_input) || changed!(old, new, escape_invalid_bytes);

        let should_rewrap_lines =
            changed!(old, new, timestamps) || changed!(old, new, show_indices);

        if changed!(old, new, bytes_per_line) {
            self.determine_bytes_per_line(new.bytes_per_line.into());
            self.correct_hex_view_scroll();
        } else if changed!(old, new, hex_view) {
            self.correct_hex_view_scroll();
        }

        if should_reconsume {
            self.reconsume_raw_buffer();
        } else if should_rewrap_lines {
            self.update_wrapped_line_heights();
        }

        self.scroll_by(0);
    }
    pub fn intentional_disconnect(&mut self) {
        self.log_handle.log_port_disconnected(true).unwrap();
        self.styled_lines.rx.clear();
        self.raw.buffer_timestamps.clear();
        self.styled_lines.tx.clear();
        self.raw.inner.clear();
        self.styled_lines.last_rx_completed = true;
    }
}

/// Returns an iterator over the given byte slice, seperated by (and excluding) the given line ending byte slice.
///
/// String slice tuple is in order of `(exclusive, inclusive/original)`.
///
/// `usize` tuple has the inclusive indices into the given slice.
///
/// If no line ending was found, emits the whole slice once.
pub fn line_ending_iter<'a>(
    bytes: &'a [u8],
    line_ending: &'a LineEnding,
) -> impl Iterator<Item = (&'a [u8], &'a [u8], (usize, usize))> {
    assert!(
        !matches!(line_ending, LineEnding::None),
        "line_ending can't be empty"
    );

    let line_ending_pos_iter = match line_ending {
        LineEnding::None => unreachable!(),
        LineEnding::Byte(byte) => Either::Right(memchr::memchr_iter(*byte, bytes)),
        LineEnding::MultiByte(finder) => {
            assert_ne!(finder.needle().len(), 0, "empty finder not allowed");
            assert_ne!(
                finder.needle().len(),
                1,
                "not allowing slower Finder search with one-byte ending"
            );
            Either::Left(finder.find_iter(bytes))
        }
    };
    let le_len = match line_ending {
        LineEnding::None => unreachable!(),
        LineEnding::Byte(_) => 1,
        LineEnding::MultiByte(finder) => finder.needle().len(),
    };

    // if not using a finder with multi-byte endings:
    // Either::Left(memchr::memmem::find_iter(bytes, line_ending_bytes))

    // let line_ending_pos_iter = if line_ending.len() == 1 {
    // } else {
    // };

    let line_ending_pos_iter = line_ending_pos_iter
        .into_iter()
        .map(|line_ending_index| (line_ending_index, false))
        .chain(std::iter::once((bytes.len(), true)));

    let mut last_index = 0;

    let slices_iter =
        line_ending_pos_iter.filter_map(move |(line_ending_index, is_final_entry)| {
            let result = if is_final_entry && last_index == bytes.len() && bytes.len() != 0 {
                return None;
            } else if is_final_entry {
                (
                    &bytes[last_index..],
                    &bytes[last_index..],
                    (last_index, bytes.len()),
                )
            } else {
                // Copy of `last_index` since we're about to modify it,
                // but we want to use the unmodified value.
                let index_copy = last_index;
                // Adding the length of the line ending to exclude it's presence
                // from the next line.
                last_index = line_ending_index + le_len;
                (
                    &bytes[index_copy..line_ending_index],
                    &bytes[index_copy..line_ending_index + le_len],
                    (index_copy, line_ending_index + le_len),
                )
            };
            Some(result)
        });

    slices_iter
}
