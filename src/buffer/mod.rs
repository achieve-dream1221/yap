use std::{cmp::Ordering, ops::Range, thread::JoinHandle};

use ansi_to_tui::{IntoText, LossyFlavor};
use bstr::{ByteSlice, ByteVec};
use buf_line::{BufLine, LineType};
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString};
use crossbeam::channel::{Receiver, Sender};
use itertools::{Either, Itertools};
use memchr::memmem::Finder;
use ratatui::{
    layout::{Rect, Size},
    style::{Color, Style, Stylize, palette::material::PINK},
    symbols,
    text::{Line, Span},
    widgets::ScrollbarState,
};
use ratatui_macros::span;
use serialport::SerialPortInfo;
use takeable::Takeable;
use tracing::{debug, error, info, warn};

#[cfg(feature = "defmt")]
use crate::buffer::defmt::DefmtDecoder;

#[cfg(feature = "defmt")]
use crate::settings::Defmt;
use crate::{
    app::Event,
    buffer::{
        buf_line::{FrameLocation, RenderSettings},
        defmt::{frame_delimiting::esp_defmt_delimit, rzcobs_decode},
        tui::COLOR_RULES_PATH,
    },
    changed,
    errors::YapResult,
    settings::{LoggingType, Rendering},
    traits::{ByteSuffixCheck, LineHelpers, interleave_by},
    tui::color_rules::ColorRules,
};

#[cfg(feature = "logging")]
use crate::settings::Logging;

mod buf_line;
mod hex_spans;
mod tui;

#[cfg(feature = "logging")]
mod logging;
#[cfg(feature = "logging")]
use logging::LoggingHandle;
#[cfg(feature = "logging")]
pub use logging::{DEFAULT_TIMESTAMP_FORMAT, LoggingEvent};
#[cfg(feature = "defmt")]
pub mod defmt;
// #[cfg(feature = "defmt")]
// pub mod ;

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

struct RangeSlice<'a> {
    range: Range<usize>,
    slice: &'a [u8],
}

impl<'a> AsRef<[u8]> for RangeSlice<'a> {
    fn as_ref(&self) -> &[u8] {
        self.slice
    }
}

impl<'a> RangeSlice<'a> {
    /// Create a [RangeSlice] from a parent buffer (likely a `Vec<u8>`)
    /// and a child slice (`&[u8]`) from within the parent buffer,
    /// populating the `range` field with the child slice's start and end indices.
    ///
    /// # Safety
    /// `child` **must** be a subslice of `parent`, i.e. both slices come from the same
    /// allocation and `child` lies **entirely** within `parent`.
    ///
    /// # Panics
    ///
    /// Debug Builds: Panics if the child slice is empty, larger than the parent,
    /// or if the child's pointers do not lie within the parent's pointer bounds.
    ///
    /// Release Builds: Assertions skipped.
    pub unsafe fn from_parent_and_child(parent: &'a [u8], child: &'a [u8]) -> Self {
        // Fail-fast checks
        debug_assert!(!parent.is_empty());
        debug_assert!(!child.is_empty());
        // TODO make debug_assert?
        debug_assert!(
            child.len() <= parent.len(),
            "child can't be larger than parent"
        );
        let parent_range = parent.as_ptr_range();
        let child_range = child.as_ptr_range();

        // Ensure child pointers lies within parent pointer bounds
        // TODO make debug_assert?
        debug_assert!(
            child_range.start >= parent_range.start,
            "child_range.start must be >= parent_range.start",
        );
        debug_assert!(
            child_range.end <= parent_range.end,
            "child_range.end must be <= parent_range.end",
        );

        // Getting difference between pointers in T-sized chunks.
        //
        // SAFETY:
        //      Trivial to ensure safety, as long as documented
        //      precondition of ensuring `child` is always a
        //      subslice of `parent`.
        let offset = unsafe { child_range.start.offset_from(parent_range.start) } as usize;

        Self {
            range: offset..offset + child.len(),
            slice: child,
        }
    }
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
    ///
    /// # Panics
    ///
    /// Panics if line_type isn't a user line.
    fn filter_user_line(&self, line_type: &LineType) -> bool {
        assert!(
            matches!(line_type, LineType::User { .. }),
            "port lines not allowed"
        );
        match self {
            UserEcho::None => false,
            UserEcho::All => true,
            UserEcho::NoBytes => !line_type.is_bytes(),
            UserEcho::NoMacros => !line_type.is_macro(),
            UserEcho::NoMacrosOrBytes => !line_type.is_bytes() && !line_type.is_macro(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum LineEnding {
    None,
    Byte(u8),
    MultiByte(Finder<'static>),
}

impl LineEnding {
    fn as_bytes(&self) -> &[u8] {
        match self {
            LineEnding::None => &[],
            LineEnding::Byte(byte) => std::slice::from_ref(byte),
            LineEnding::MultiByte(finder) => finder.needle(),
        }
    }

    fn escaped_from(&self, buffer: &[u8]) -> Option<CompactString> {
        if buffer.has_line_ending(self) {
            Some(self.as_bytes().escape_bytes().to_compact_string())
        } else {
            None
        }
    }
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

enum BufferSlice<'a> {
    PotentialText {
        slice: &'a [u8],
        continuing_last_text: bool,
    },
}

struct RawBuffer {
    inner: Vec<u8>,
    /// Time-tagged indexes into `raw_buffer`, from each input from the port.
    buffer_timestamps: Vec<(usize, DateTime<Local>)>,
    consumed_up_to: usize,
}

impl RawBuffer {
    fn with_capacities(raw: usize, timestamps: usize) -> Self {
        Self {
            buffer_timestamps: Vec::with_capacity(timestamps),
            inner: Vec::with_capacity(raw),
            consumed_up_to: 0,
        }
    }
    fn reset(&mut self) {
        self.inner.clear();
        self.inner.shrink_to(1024);
        self.buffer_timestamps.clear();
        self.buffer_timestamps.shrink_to(1024);
        self.consumed_up_to = 0;
    }
    fn feed(&mut self, new: &[u8], timestamp: DateTime<Local>) {
        // warn!("fed {} bytes", new.len());
        self.buffer_timestamps.push((self.inner.len(), timestamp));
        self.inner.extend(new);
    }
    fn consumed(&mut self, amount: usize) {
        self.consumed_up_to += amount;
    }
    fn next_raw(&self) -> Option<&[u8]> {
        let newest = &self.inner[self.consumed_up_to..];
        if newest.is_empty() {
            None
        } else {
            // self.consumed_up_to = self.inner.len();
            Some(newest)
        }
    }
    fn range(&self, range: Range<usize>) -> Option<&[u8]> {
        let len = self.inner.len();
        if range.end <= len {
            Some(&self.inner[range])
        } else {
            None
        }
    }
    /// Returns (index_in_buffer, raw/defmt slice)
    ///
    /// Returns None if either a defmt frame is incomplete, or there is no new data to give.
    fn next_frame(&self) -> Option<(usize, defmt::DefmtDelimitedSlice)> {
        let start_index = self.consumed_up_to;
        let newest = &self.inner[start_index..];
        if newest.is_empty() {
            return None;
        }
        let (rest, acting_on) = match esp_defmt_delimit(newest) {
            Ok((rest, acting_on)) => (rest, acting_on),
            // Only returned if there's an incomplete frame.
            Err(e) => {
                error!("ugh");
                return None;
            }
        };
        // let unconsumed = rest.len();
        // self.consumed_up_to = self.inner.len() - unconsumed;

        Some((start_index, acting_on))

        // match acting_on {
        //     defmt::DefmtDelimitedSlice::DefmtRzcobsPrefixed { inner: packet, .. }
        //     | defmt::DefmtDelimitedSlice::DefmtRzcobs(packet) => {
        //         let uncompressed = rzcobs_decode(packet).unwrap();
        //         let (frame, consumed) = self
        //             .defmt_decoder
        //             .as_ref()
        //             .unwrap()
        //             .table
        //             .decode(&uncompressed)
        //             .unwrap();
        //         debug!("{}", frame.display(false));
        //     }
        //     defmt::DefmtDelimitedSlice::Raw(raw) => self.consume_potentially_text(raw, known_time),
        // }

        // self.raw.consumed_up_to != self.raw.inner.len()
    }
}

struct StyledLines {
    rx: Vec<BufLine>,
    // dont_append_to_last_rx: bool,
    tx: Vec<BufLine>,
}

// impl StyledLines {
//     pub fn consume_potentially_text(
//         &mut self,
//         new_slice: FatterPointer,
//         known_time: DateTime<Local>,
//         raw_buffer: &RawBuffer,
//         line_ending: &LineEnding,
//         rendering: &Rendering,
//         color_rules: &ColorRules,
//     ) {
//         let lossy_flavor = if rendering.escape_invalid_bytes {
//             LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
//         } else {
//             LossyFlavor::replacement_char()
//         };

//         let FatterPointer {
//             index_in_buffer,
//             slice,
//         } = new_slice;

//         let mut incomplete_line_start = {
//             match self.rx.last() {
//                 Some(
//                     bf @ BufLine {
//                         line_type:
//                             LineType::Port {
//                                 escaped_line_ending: None,
//                             },
//                         ..
//                     },
//                 ) => Some(bf.raw_buffer_index),

//                 _ => None,
//             }
//         };

//         if let Ok(_) = self.tx.binary_search_by(|tx| {}) {}

//         // Consume flag.
//         // if self.dont_append_to_last_rx {
//         //     self.dont_append_to_last_rx = false;
//         //     incomplete_line_start = None;
//         // }

//         let allowed_bytes = if let Some(last_line_index) = incomplete_line_start {
//             &raw_buffer.inner[last_line_index..new_slice.as_ref().len()]
//         } else {
//             new_slice.as_ref()
//         };

//         let mut separated_lines = line_ending_iter(allowed_bytes, line_ending);

//         let first = separated_lines.next();

//         let make_a_line = |trunc: &[u8], orig: &[u8], start_index: usize| {
//             let mut line = match trunc.into_line_lossy(Style::new(), lossy_flavor) {
//                 Ok(line) => line,
//                 Err(_) => {
//                     error!("ansi-to-tui failed to parse input! Using unstyled text.");
//                     Line::from(String::from_utf8_lossy(trunc).to_string())
//                 }
//             };

//             let line_opt = self.color_rules.apply_onto(trunc, line);

//             if let Some(line) = line_opt {
//                 Some(BufLine::port_text_line(
//                     line,
//                     orig,
//                     start_index,
//                     self.last_terminal_size.width,
//                     &self.rendering,
//                     &self.line_ending,
//                     known_time,
//                 ))
//             } else {
//                 None
//             }
//         };

//         if let Some((first_trunc, first_orig, first_indices)) = first {
//             if incomplete_line_start.is_some() {
//                 let last_line = self.rx.last_mut().expect("can't append to nothing");

//                 let trunc = &allowed_bytes[..first_trunc.len()];
//                 let orig = &allowed_bytes[..first_orig.len()];
//                 // info!("AAAFG: {:?}", slice);

//                 let mut line = match trunc.into_line_lossy(Style::new(), lossy_flavor) {
//                     Ok(line) => line,
//                     Err(_) => {
//                         error!("ansi-to-tui failed to parse input! Using unstyled text.");
//                         Line::from(String::from_utf8_lossy(trunc).to_string())
//                     }
//                 };
//                 // debug!(
//                 //     "buf_index: {last_index}, update: {line}",
//                 //     line = line
//                 //         .spans
//                 //         .iter()
//                 //         .map(|s| s.content.as_ref())
//                 //         .join("")
//                 //         .escape_default()
//                 // );

//                 // if line.width() >= 5 {
//                 //     line.style_slice(1..3, Style::new().red().italic());
//                 // }

//                 let line_opt = self.color_rules.apply_onto(trunc, line);

//                 if let Some(line) = line_opt {
//                     last_line.update_line(
//                         line,
//                         orig,
//                         self.last_terminal_size.width,
//                         &self.rendering,
//                         &self.line_ending,
//                     );
//                 } else {
//                     _ = self.styled_lines.rx.pop();
//                     // self.styled_lines.last_rx_completed = true;
//                     // last_line.clear_line();
//                 }
//             } else {
//                 if let Some(line) = make_a_line(first_trunc, first_orig) {
//                     self.rx.push(line);
//                 }
//             }
//         }

//         // any other lines:
//         for (trunc, orig, indices) in separated_lines {}

//         // let this_rx_completed = self.raw.inner.has_line_ending(&self.line_ending);

//         // self.styled_lines.last_rx_completed = this_rx_completed;
//     }
// }

pub struct Buffer {
    raw: RawBuffer,
    styled_lines: StyledLines,
    last_rx_was_complete: bool,

    /// The last-known size of the area given to render the buffer in
    last_terminal_size: Size,

    pub state: BufferState,

    rendering: Rendering,

    line_ending: LineEnding,

    color_rules: ColorRules,

    #[cfg(feature = "logging")]
    pub log_handle: LoggingHandle,
    #[cfg(feature = "logging")]
    log_thread: Takeable<JoinHandle<()>>,
    #[cfg(feature = "logging")]
    log_settings: Logging,
    #[cfg(feature = "defmt")]
    pub defmt_decoder: Option<DefmtDecoder>,
    #[cfg(feature = "defmt")]
    defmt_settings: Defmt,
    // #[cfg(feature = "defmt")]
    // frame_delimiter: FrameDelimiter,
}

#[cfg(feature = "logging")]
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
    pub fn new(
        line_ending: &[u8],
        rendering: Rendering,
        #[cfg(feature = "logging")] logging: Logging,
        #[cfg(feature = "logging")] event_tx: Sender<Event>,
        #[cfg(feature = "defmt")] defmt: Defmt,
    ) -> Self {
        let line_ending: LineEnding = line_ending.into();
        #[cfg(feature = "logging")]
        let (log_handle, log_thread) =
            LoggingHandle::new(line_ending.clone(), logging.clone(), event_tx);

        Self {
            raw: RawBuffer {
                inner: Vec::with_capacity(1024),
                buffer_timestamps: Vec::with_capacity(1024),
                consumed_up_to: 0,
            },
            styled_lines: StyledLines {
                rx: Vec::with_capacity(1024),
                // dont_append_to_last_rx: true,
                tx: Vec::with_capacity(1024),
            },
            last_rx_was_complete: true,
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
            color_rules: ColorRules::load_from_file(COLOR_RULES_PATH),
            #[cfg(feature = "logging")]
            log_handle,
            #[cfg(feature = "logging")]
            log_thread: Takeable::new(log_thread),
            #[cfg(feature = "logging")]
            log_settings: logging,
            #[cfg(feature = "defmt")]
            defmt_decoder: None,
            #[cfg(feature = "defmt")]
            defmt_settings: defmt,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    pub fn append_user_bytes(
        &mut self,
        bytes: &[u8],
        line_ending: &[u8],
        is_macro: bool,
        sensitive: bool,
    ) {
        let now = Local::now();
        let user_span = span!(Color::DarkGray; "BYTE> ");

        let text = if !sensitive {
            let text: Span = bytes
                .iter()
                .chain(line_ending.iter())
                .map(|b| format!("\\x{:02X}", b))
                .join("")
                .into();
            let text = text.dark_gray().italic().bold();

            text
        } else {
            span!(Style::new().dark_gray(); "*".repeat(bytes.len()))
        };

        let line = Line::from(vec![user_span, text]);

        let combined: Vec<_> = bytes.iter().chain(line_ending.iter()).map(|b| *b).collect();
        let line_ending: LineEnding = line_ending.into();

        // line.spans.insert(0, user_span.clone());
        // line.style_all_spans(Color::DarkGray.into());
        // let user_buf_line = BufLine::new_with_line(
        //     line,
        //     bytes,
        //     self.raw.inner.len(), // .max(1)
        //     self.last_terminal_size.width,
        //     &self.rendering,
        //     &self.line_ending,
        //     now,
        //     LineType::User {
        //         is_bytes: true,
        //         is_macro,
        //         reloggable_raw: combined,
        //     },
        // );
        let user_buf_line = BufLine::user_line(
            line,
            self.raw.inner.len(),
            self.last_terminal_size.width,
            self.line_render_settings(),
            &line_ending,
            now,
            true,
            is_macro,
            &combined,
        );

        self.last_rx_was_complete = self
            .rendering
            .echo_user_input
            .filter_user_line(&user_buf_line.line_type)
            || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));

        #[cfg(feature = "logging")]
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

    pub fn append_user_text(
        &mut self,
        text: &str,
        line_ending: &[u8],
        is_macro: bool,
        sensitive: bool,
    ) {
        let now = Local::now();
        // let escaped_line_ending = line_ending.escape_bytes().to_string();
        // let escaped_chained: Vec<u8> = text
        //     .as_bytes()
        //     .iter()
        //     .chain(escaped_line_ending.as_bytes().iter())
        //     .map(|i| *i)
        //     .collect();

        let line_ending: LineEnding = line_ending.into();

        let user_span = span!(Color::DarkGray;"USER> ");
        // let Text { lines, .. } = text;
        // TODO HANDLE MULTI-LINE USER INPUT AAAA
        for (trunc, orig, _indices) in line_ending_iter(text.as_bytes(), &line_ending) {
            // not sure if i want to ansi-style user text?
            // let mut line = match trunc.into_line_lossy(Style::new()) {
            //     Ok(line) => line,
            //     Err(_) => {
            //         error!("ansi-to-tui failed to parse input! Using unstyled text.");
            //         Line::from(String::from_utf8_lossy(trunc).to_string())
            //     }
            // };

            let line = if !sensitive {
                let mut line = Line::from(String::from_utf8_lossy(trunc).to_string());
                line.spans.insert(0, user_span.clone());
                line.style_all_spans(Color::DarkGray.into());
                line
            } else {
                let text = span!(Style::new().dark_gray(); "*".repeat(trunc.len()));
                let line = Line::from(vec![user_span.clone(), text]);
                line
            };

            // let user_buf_line = BufLine::new_with_line(
            //     line,
            //     orig,
            //     self.raw.inner.len(), // .max(1)
            //     self.last_terminal_size.width,
            //     &self.rendering,
            //     &self.line_ending,
            //     now,
            //     LineType::User {
            //         is_bytes: false,
            //         is_macro,
            //         reloggable_raw: orig.to_owned(),
            //     },
            // );
            let user_buf_line = BufLine::user_line(
                line,
                self.raw.inner.len(),
                self.last_terminal_size.width,
                self.line_render_settings(),
                &line_ending,
                now,
                false,
                is_macro,
                orig,
            );
            // Used to be out of the for loop.
            self.last_rx_was_complete = self
                .rendering
                .echo_user_input
                .filter_user_line(&user_buf_line.line_type)
                || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));

            #[cfg(feature = "logging")]
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

    // Forced to use Vec<u8> for now
    pub fn fresh_rx_bytes(&mut self, bytes: &mut Vec<u8>) {
        let now = Local::now();
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);
        #[cfg(feature = "logging")]
        self.log_handle.log_rx_bytes(now, bytes.clone()).unwrap();

        self.raw.feed(&bytes, now);

        let meow = std::time::Instant::now();
        self.consume_latest_bytes(now);
        // error!("{:?}", meow.elapsed());

        // self.raw.inner.extend(bytes.iter());
        // while self.consume_latest_bytes(Some(now)) {
        //     ();
        // }
    }

    fn consume_latest_bytes(&mut self, timestamp: DateTime<Local>) {
        while let Some((index_in_buffer, meow)) = self.raw.next_frame() {
            let slice = match meow {
                defmt::DefmtDelimitedSlice::Raw(slice) => slice,
                defmt::DefmtDelimitedSlice::DefmtRzcobs { raw, inner } => {
                    // nevermind, empty inner packets arent possible now that we skip leading 0x00s
                    // if inner.is_empty() {
                    //     self.styled_lines.rx.push(BufLine::port_text_line(
                    //         Line::raw("Empty defmt packet!"),
                    //         unsafe { RangeSlice::from_parent_and_child(&self.raw.inner, raw) },
                    //         self.last_terminal_size.width,
                    //         &self.rendering,
                    //         &self.line_ending,
                    //         timestamp,
                    //     ));

                    //     self.last_rx_was_complete = true;
                    //     self.raw.consumed(meow.raw_len());
                    //     continue;
                    // }

                    #[derive(Debug)]
                    enum DecodeFailReason {
                        NoDecoder,
                        RzcobsDecompress,
                        DefmtDecode,
                    }

                    let mut failed_decode = |reason: DecodeFailReason| {
                        let mut text =
                            format!("Couldn't decode defmt rzcobs packet ({reason:?}): ");
                        text.extend(inner.iter().map(|b| format!("{b:02X}")));

                        let render_settings = RenderSettings {
                            rendering: &self.rendering,
                            defmt: &self.defmt_settings,
                        };

                        self.styled_lines.rx.push(BufLine::port_text_line(
                            Line::raw(text),
                            unsafe { RangeSlice::from_parent_and_child(&self.raw.inner, raw) },
                            self.last_terminal_size.width,
                            render_settings,
                            &self.line_ending,
                            timestamp,
                        ));
                    };

                    // self.styled_lines.dont_append_to_last_rx = true;

                    // let-chains my beloved, where art thou
                    if let Some(decoder) = &self.defmt_decoder {
                        if let Ok(uncompressed) = rzcobs_decode(inner) {
                            if let Ok((frame, consumed)) = decoder.table.decode(&uncompressed) {
                                // choosing to skip pushing instead of filtering
                                // to keep consistent with other logic that expects
                                // invisible lines to not be pushed,
                                // and to skip further handling for those lines.
                                let commit_frame = match frame.level() {
                                    None => true,
                                    Some(level) => {
                                        self.defmt_settings.max_log_level
                                            <= crate::settings::Level::from(level)
                                    }
                                };
                                if !commit_frame {
                                    self.raw.consumed(meow.raw_len());
                                    continue;
                                }

                                // let meow = std::time::Instant::now();
                                // error!("{:?}", meow.elapsed());
                                // debug!("{frame:#?}");
                                let loc_opt = decoder
                                    .locations
                                    .as_ref()
                                    .and_then(|locs| locs.get(&frame.index()))
                                    .map(FrameLocation::from);

                                let message = frame.display_message().to_string();
                                let message_lines = message.lines();

                                let device_timestamp = frame.display_timestamp();
                                let device_timestamp_ref = device_timestamp
                                    .as_ref()
                                    .map(|ts| ts as &dyn std::fmt::Display);

                                for line in message_lines {
                                    let mut message_line = Line::default();
                                    message_line.push_span(Span::raw(line));

                                    if let Some(line) =
                                        self.color_rules.apply_onto(line.as_bytes(), message_line)
                                    {
                                        let owned_spans: Vec<Span<'static>> = line
                                            .into_iter()
                                            .map(|s| match s.content {
                                                std::borrow::Cow::Owned(owned) => Span {
                                                    content: std::borrow::Cow::Owned(owned),
                                                    ..s
                                                },
                                                std::borrow::Cow::Borrowed(borrowed) => Span {
                                                    content: std::borrow::Cow::Owned(
                                                        borrowed.to_string(),
                                                    ),
                                                    ..s
                                                },
                                            })
                                            .collect();

                                        let owned_line = Line {
                                            spans: owned_spans,
                                            ..Default::default()
                                        };

                                        self.styled_lines.rx.push(BufLine::port_defmt_line(
                                            owned_line,
                                            unsafe {
                                                RangeSlice::from_parent_and_child(
                                                    &self.raw.inner,
                                                    raw,
                                                )
                                            },
                                            self.last_terminal_size.width,
                                            self.line_render_settings(),
                                            frame.level(),
                                            device_timestamp_ref,
                                            loc_opt.clone(),
                                            timestamp,
                                        ));
                                    }
                                }
                            } else {
                                failed_decode(DecodeFailReason::DefmtDecode);
                            }
                        } else {
                            failed_decode(DecodeFailReason::RzcobsDecompress);
                        }
                    } else {
                        failed_decode(DecodeFailReason::NoDecoder);
                    }
                    self.last_rx_was_complete = true;
                    self.raw.consumed(meow.raw_len());
                    continue;
                }
            };

            let mut can_append_to_line = !self.last_rx_was_complete;

            for (trunc, orig, indices) in line_ending_iter(slice, &self.line_ending.clone()) {
                // index = self.raw.inner.len();

                // if let Some(index) = self.index_of_incomplete_line.take() {
                if can_append_to_line {
                    can_append_to_line = false;
                    let last_line = self
                        .styled_lines
                        .rx
                        .last_mut()
                        .expect("can't append to nothing");
                    let last_index = last_line.index_in_buffer();
                    // assert_eq!(last_index, index);

                    let trunc = last_index..index_in_buffer + trunc.len();
                    let trunc = self.raw.range(trunc).unwrap();
                    let orig = last_index..index_in_buffer + orig.len();
                    let orig = self.raw.range(orig).unwrap();
                    // debug!("Appendo from {last_index}! trunc: {trunc:#?} orig: {orig:#?}");

                    // info!("AAAFG: {:?}", slice);
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

                    let line_opt = self.color_rules.apply_onto(trunc, line);

                    let render_settings = RenderSettings {
                        rendering: &self.rendering,
                        defmt: &self.defmt_settings,
                    };

                    if let Some(line) = line_opt {
                        last_line.update_line(
                            line,
                            unsafe { RangeSlice::from_parent_and_child(&self.raw.inner, orig) },
                            self.last_terminal_size.width,
                            render_settings,
                            &self.line_ending,
                        );
                    } else {
                        _ = self.styled_lines.rx.pop();
                        // self.styled_lines.last_rx_completed = true;
                        // last_line.clear_line();
                    }
                    self.last_rx_was_complete = orig.has_line_ending(&self.line_ending);
                    continue;
                }

                if let Some(new_bufline) = self.slice_as_port_text(
                    unsafe { RangeSlice::from_parent_and_child(&self.raw.inner, orig) },
                    timestamp,
                ) {
                    self.styled_lines.rx.push(new_bufline);
                }
                self.last_rx_was_complete = orig.has_line_ending(&self.line_ending);
            }

            // self.styled_lines.consume_potentially_text(
            //     raw_fatty,
            //     now,
            //     &self.raw,
            //     &self.line_ending,
            //     &self.rendering,
            //     &self.color_rules,
            // );

            // 🥩🥩🥩 Consume the Meat 🥩🥩🥩
            self.raw.consumed(meow.raw_len());
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

        // Taking these variables out of `self` temporarily to allow running &mut self methods while holding
        // references to these.
        let user_timestamps: Vec<_> = self
            .styled_lines
            .tx
            .iter()
            .map(|b| {
                let LineType::User {
                    is_bytes, is_macro, ..
                } = &b.line_type
                else {
                    unreachable!();
                };

                (
                    LineType::User {
                        is_bytes: *is_bytes,
                        is_macro: *is_macro,
                        reloggable_raw: Vec::new(),
                        escaped_line_ending: None,
                    },
                    b.raw_buffer_index,
                    b.timestamp,
                )
            })
            .collect();
        let orig_buf_len = self.raw.inner.len();
        let timestamps_len = self.raw.buffer_timestamps.len();
        let rx_buffer = std::mem::replace(
            &mut self.raw,
            RawBuffer::with_capacities(orig_buf_len, timestamps_len),
        );
        let user_echo = self.rendering.echo_user_input.clone();

        // No lines to append to.
        self.last_rx_was_complete = true;

        // Getting all time-tagged indices in the buffer where either
        // 1. Data came in through the port
        // 2. The user sent data
        let interleaved_points = interleave_by(
            rx_buffer
                .buffer_timestamps
                .iter()
                .map(|(index, timestamp)| (*index, *timestamp, false))
                // Add a "finale" element to capture any remaining buffer, always placed at the end.
                .chain(std::iter::once((orig_buf_len, Local::now(), false))),
            user_timestamps
                .into_iter()
                // If a user line isn't visible, ignore it when taking external new-lines into account.
                .filter(|(line_type, _, _)| user_echo.filter_user_line(line_type))
                .map(|(_, index, timestamp)| (index, timestamp, true)),
            // Interleaving by sorting in order of raw_buffer_index, if they're equal, then whichever has a sooner timestamp.
            |port, user| match port.0.cmp(&user.0) {
                Ordering::Equal => port.1 <= user.1,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        );

        let mut new_length = 0;
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
                        &rx_buffer.inner[start_index..end_index],
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
                self.last_rx_was_complete = true;
                continue;
            }
            self.raw.feed(slice, timestamp);
            self.consume_latest_bytes(timestamp);
            // info!(
            //     "Getting {le} slices from [{slice_start}..{slice_end}], {timestamp}, {was_user_line}",
            //     le = line_ending.escape_debug()
            // );
            // for (trunc, orig, (orig_start, orig_end)) in line_ending_iter(slice, &line_ending) {
            // info!(
            //     "trunc: {trunc_len}, orig: {orig_len}. [{start}..{end}]",
            //     trunc_len = trunc.len(),
            //     orig_len = orig.len(),
            //     start = orig_start + slice_start,
            //     end = orig_end + slice_start,
            // );

            // while self.consume_latest_bytes(Some(timestamp)) {}
            new_length += slice.len();
            // }
        }

        // defmt_delimit(meow);

        // Asserting that our work seems correct.
        assert_eq!(
            orig_buf_len,
            self.raw.inner.len(),
            "Buffer size should not have changed during reconsumption."
        );
        assert_eq!(
            new_length, orig_buf_len,
            "Iterator's slices should have same total length as raw buffer."
        );
        assert_eq!(
            self.raw.buffer_timestamps, rx_buffer.buffer_timestamps,
            "RawBuffers should have identical buffer_timestamps."
        );
        self.styled_lines.rx.windows(2).for_each(|lines| {
            assert!(
                lines[0].raw_buffer_index < lines[1].raw_buffer_index,
                "Port lines should be in exact ascending order by index."
            )
        });

        // Returning variables we stole back to self.
        // (excluding the raw buffer since that got reconsumed gradually back into self)
        // self.raw.buffer_timestamps = rx_timestamps;
        // self.styled_lines.tx = user_lines;
        self.scroll_by(0);
    }

    // pub fn update_line_ending(&mut self, line_ending: &str) {
    pub fn update_line_ending(&mut self, line_ending: &[u8]) {
        if self.line_ending != line_ending {
            self.line_ending = line_ending.into();
            self.reconsume_raw_buffer();
            #[cfg(feature = "logging")]
            self.log_handle
                .update_line_ending(self.line_ending.clone())
                .unwrap();
            #[cfg(feature = "logging")]
            self.clear_and_relog_buffers(self.log_settings.log_user_input);
        }
    }
    pub fn update_render_settings(&mut self, rendering: Rendering) {
        let old = std::mem::replace(&mut self.rendering, rendering);
        let new = &self.rendering;
        let should_reconsume = changed!(old, new, echo_user_input, escape_invalid_bytes);

        let should_rewrap_lines = changed!(old, new, timestamps, show_indices, show_line_ending);

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
    #[cfg(feature = "defmt")]
    pub fn update_defmt_settings(&mut self, defmt: Defmt) {
        let old = std::mem::replace(&mut self.defmt_settings, defmt);
        let new = &self.defmt_settings;
        let should_reconsume = changed!(old, new, defmt_parsing, max_log_level);

        let should_rewrap_lines = changed!(
            old,
            new,
            device_timestamp,
            show_file,
            show_module,
            show_line_number
        );

        if should_reconsume {
            self.reconsume_raw_buffer();
        } else if should_rewrap_lines {
            self.update_wrapped_line_heights();
        }

        self.scroll_by(0);
    }
    #[cfg(feature = "logging")]
    fn clear_and_relog_buffers(&mut self, with_user_input: bool) {
        self.log_handle.clear_current_logs().unwrap();

        let interleaved_points = interleave_by(
            self.raw
                .buffer_timestamps
                .iter()
                .map(|(index, timestamp)| (*index, *timestamp, None))
                // Add a "finale" element to capture any remaining buffer, always placed at the end.
                .chain(std::iter::once((self.raw.inner.len(), Local::now(), None))),
            self.styled_lines
                .tx
                .iter()
                // If a user line isn't visible, ignore it when taking external new-lines into account.
                .filter(|_| with_user_input)
                .map(|b| {
                    let LineType::User {
                        reloggable_raw: raw,
                        ..
                    } = &b.line_type
                    else {
                        unreachable!();
                    };
                    (b.raw_buffer_index, b.timestamp, Some(raw))
                }),
            // Interleaving by sorting in order of raw_buffer_index, if they're equal, then whichever has a sooner timestamp.
            |port, user| match port.0.cmp(&user.0) {
                Ordering::Equal => port.1 <= user.1,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        );

        let buffer_slices = interleaved_points
            .tuple_windows()
            // Filtering out some empty slices, unless they indicate a user event.
            .filter(|((start_index, _, user_line_buffer), (end_index, _, _))| {
                start_index != end_index || user_line_buffer.is_some()
            })
            // Building the parent slices (pre-newline splitting)
            .map(
                |((start_index, timestamp, user_line_buffer), (end_index, _, _))| {
                    (
                        &self.raw.inner[start_index..end_index],
                        timestamp,
                        user_line_buffer,
                        (start_index, end_index),
                    )
                },
            );

        for (slice, timestamp, user_line_buffer, (slice_start, slice_end)) in buffer_slices {
            if let Some(raw) = user_line_buffer {
                self.log_handle
                    .log_tx_bytes(
                        timestamp,
                        raw.to_owned(),
                        self.line_ending.as_bytes().to_owned(),
                    )
                    .unwrap();
            } else {
                self.log_handle
                    .log_rx_bytes(timestamp, slice.to_owned())
                    .unwrap();
            }
        }
    }
    #[cfg(feature = "logging")]
    pub fn update_logging_settings(
        &mut self,
        logging: Logging,
        current_port: Option<SerialPortInfo>,
    ) {
        let local_copy = logging.clone();
        let old = std::mem::replace(&mut self.log_settings, local_copy);
        let new = &self.log_settings;

        let resend_needed = changed!(old, new, log_user_input)
            || changed!(old, new, log_file_type)
            || changed!(old, new, timestamp);

        // Meh. Arbitrary decision but I don't want to resend everything if this changes.
        // Especially since I don't currently log connection events to place them back in retroactively.
        // changed!(old, new, log_connection_events)

        self.log_handle.update_settings(logging).unwrap();

        if resend_needed && !self.raw.inner.is_empty() {
            let log_user_input = new.log_user_input;
            self.clear_and_relog_buffers(log_user_input);
        }
    }
    pub fn intentional_disconnect_clear(&mut self) {
        #[cfg(feature = "logging")]
        self.log_handle.log_port_disconnected(true).unwrap();

        self.styled_lines.rx.clear();
        self.styled_lines.rx.shrink_to(1024);

        self.styled_lines.tx.clear();
        self.styled_lines.tx.shrink_to(1024);

        self.last_rx_was_complete = true;

        self.raw.reset();
    }
    // Returns None if the slice would be fully hidden by the color rules.
    fn slice_as_port_text(
        &self,
        full_slice: RangeSlice,
        known_time: DateTime<Local>,
    ) -> Option<BufLine> {
        let lossy_flavor = if self.rendering.escape_invalid_bytes {
            LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
        } else {
            LossyFlavor::replacement_char()
        };

        let RangeSlice {
            range,
            slice: original,
        } = &full_slice;

        let truncated = if original.has_line_ending(&self.line_ending) {
            &original[..original.len() - self.line_ending.as_bytes().len()]
        } else {
            original
        };

        let line = match truncated.into_line_lossy(Style::new(), lossy_flavor) {
            Ok(line) => line,
            Err(_) => {
                error!("ansi-to-tui failed to parse input! Using unstyled text.");
                Line::from(String::from_utf8_lossy(truncated).to_string())
            }
        };

        let line_opt = self.color_rules.apply_onto(truncated, line);

        if let Some(line) = line_opt {
            Some(BufLine::port_text_line(
                line,
                full_slice,
                self.last_terminal_size.width,
                self.line_render_settings(),
                &self.line_ending,
                known_time,
            ))
        } else {
            None
        }
    }
}

/// Returns an iterator over the given byte slice, seperated by (and excluding) the given line ending byte slice.
///
/// String slice tuple is in order of `(exclusive, inclusive/original)`.
///
/// `usize` tuple has the inclusive indices into the given slice.
///
/// If no line ending was found, emits the whole slice once.
#[inline(always)]
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
