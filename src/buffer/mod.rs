#[cfg(feature = "defmt")]
use std::sync::Arc;
use std::{borrow::Cow, cmp::Ordering, ops::Range, thread::JoinHandle};

use ansi_to_tui::{IntoText, LossyFlavor};
use bstr::{ByteSlice, ByteVec};
use buf_line::{BufLine, LineType};
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString};
use crossbeam::channel::Sender;
use itertools::{Either, Itertools};
use memchr::memmem::Finder;
use ratatui::{
    layout::Size,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::ScrollbarState,
};
use ratatui_macros::span;
use takeable::Takeable;
use tracing::{debug, error, warn};

#[cfg(feature = "defmt")]
use crate::buffer::defmt::DefmtDecoder;

#[cfg(feature = "defmt")]
use crate::settings::Defmt;
use crate::{
    app::Event,
    buffer::{
        buf_line::{BufLineKit, RenderSettings},
        tui::COLOR_RULES_PATH,
    },
    changed, config_adjacent_path,
    errors::HandleResult,
    settings::Rendering,
    traits::{ByteSuffixCheck, LineHelpers, interleave_by},
    tui::color_rules::{ColorRuleError, ColorRules},
};

#[cfg(feature = "defmt")]
use crate::buffer::{
    buf_line::FrameLocation,
    defmt::{DefmtPacketError, rzcobs_decode},
};
#[cfg(feature = "defmt")]
use crate::settings::DefmtSupport;

#[cfg(feature = "logging")]
use crate::settings::Logging;

mod buf_line;
mod hex_spans;
pub use hex_spans::*;
mod tui;

#[cfg(feature = "logging")]
mod logging;
#[cfg(feature = "logging")]
pub use logging::LoggingHandle;
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

#[derive(Debug, Clone)]
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
    /// Create a [`RangeSlice`] from a parent buffer (likely a `Vec<u8>`)
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
        debug_assert!(
            child.len() <= parent.len(),
            "child can't be larger than parent"
        );
        let parent_range = parent.as_ptr_range();
        let child_range = child.as_ptr_range();

        // Ensure child pointers lies within parent pointer bounds
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

#[allow(clippy::large_enum_variant)]
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
        } else if value.len() == 1 {
            LineEnding::Byte(value[0])
        } else {
            let finder = Finder::new(value).into_owned();
            LineEnding::MultiByte(finder)
        }
    }
}

// TODO tests for buffer behavior with new lines
// (i broke it once before with tests passing, so, bleh)

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
    fn range(&self, range: Range<usize>) -> Option<&[u8]> {
        let len = self.inner.len();
        if range.end <= len {
            Some(&self.inner[range])
        } else {
            None
        }
    }
    fn next_slice_raw(&self) -> Option<(usize, DelimitedSlice)> {
        let start_index = self.consumed_up_to;
        let newest = &self.inner[start_index..];
        if newest.is_empty() {
            None
        } else {
            Some((start_index, DelimitedSlice::Raw(newest)))
        }
    }
    #[cfg(feature = "defmt")]
    fn next_slice(&self, defmt_support: DefmtSupport) -> Option<(usize, DelimitedSlice)> {
        match defmt_support {
            DefmtSupport::FramedRzcobs => self.next_slice_defmt_rzcobs(true),
            DefmtSupport::UnframedRzcobs => self.next_slice_defmt_rzcobs(false),
            DefmtSupport::Raw => self.next_slice_defmt_raw(),
            DefmtSupport::Disabled => self.next_slice_raw(),
        }
    }
    #[cfg(feature = "defmt")]
    fn next_slice_defmt_raw(&self) -> Option<(usize, DelimitedSlice)> {
        let start_index = self.consumed_up_to;
        let newest = &self.inner[start_index..];
        if newest.is_empty() {
            None
        } else {
            Some((start_index, DelimitedSlice::DefmtRaw(newest)))
        }
    }
    #[cfg(feature = "defmt")]
    /// Returns (index_in_buffer, raw/defmt slice)
    ///
    /// Returns None if either a defmt frame is incomplete, or there is no new data to give.
    fn next_slice_defmt_rzcobs(&self, esp_println_framed: bool) -> Option<(usize, DelimitedSlice)> {
        let start_index = self.consumed_up_to;
        let newest = &self.inner[start_index..];
        if newest.is_empty() {
            return None;
        }

        let slice_fn = if esp_println_framed {
            defmt::frame_delimiting::esp_println_delimited
        } else {
            defmt::frame_delimiting::zero_delimited
        };

        let slice = match slice_fn(newest) {
            // `rest` unused, up to caller to call Self::consumed() with,
            // the length of `slice` to avoid reconsumption.
            Ok((_rest, slice)) => slice,
            // Only returned if there's an incomplete frame.
            Err(_) => {
                return None;
            }
        };

        Some((start_index, slice))
    }
}

#[derive(Debug, PartialEq)]
pub enum DelimitedSlice<'a> {
    #[cfg(feature = "defmt")]
    /// Used by framed inputs, such as from esp-println.
    DefmtRzcobs {
        /// Complete original slice, containing prefix and terminator.
        raw: &'a [u8],
        /// (Supposedly) rzCOBS packet, stripped of prefix and terminator.
        inner: &'a [u8],
    },
    #[cfg(feature = "defmt")]
    /// Use if ELF has raw encoding enabled (no rzCOBS compression).
    DefmtRaw(&'a [u8]),
    /// Non-defmt input, either junk data or plain ASCII/UTF-8 logs.
    Raw(&'a [u8]),
}

impl DelimitedSlice<'_> {
    pub fn raw_len(&self) -> usize {
        match self {
            #[cfg(feature = "defmt")]
            DelimitedSlice::DefmtRzcobs { raw, .. } => raw.len(),
            #[cfg(feature = "defmt")]
            DelimitedSlice::DefmtRaw(raw) => raw.len(),
            DelimitedSlice::Raw(raw) => raw.len(),
        }
    }
}

struct StyledLines {
    rx: Vec<BufLine>,
    last_rx_was_complete: bool,
    tx: Vec<BufLine>,
}

impl StyledLines {
    #[cfg(feature = "defmt")]
    fn failed_decode(
        &mut self,
        delimited_slice: DelimitedSlice,
        reason: DefmtPacketError,
        kit: BufLineKit,
        line_ending: &LineEnding,
    ) {
        let DelimitedSlice::DefmtRzcobs { raw, inner } = delimited_slice else {
            unreachable!()
        };

        let mut text = format!("Couldn't decode defmt rzcobs packet ({reason}): ");
        text.extend(inner.iter().map(|b| format!("{b:02X}")));

        self.rx
            .push(BufLine::port_text_line(Line::raw(text), kit, line_ending));
    }
    fn consume_as_text(
        &mut self,
        raw_buffer: &RawBuffer,
        color_rules: &ColorRules,
        index_in_buffer: usize,
        delimited_slice: DelimitedSlice,
        kit: BufLineKit,
        line_ending: &LineEnding,
    ) {
        let mut can_append_to_line = !self.last_rx_was_complete;

        let DelimitedSlice::Raw(slice) = delimited_slice else {
            unreachable!()
        };

        for (trunc, orig, indices) in line_ending_iter(slice, line_ending) {
            // index = self.raw.inner.len();

            // if let Some(index) = self.index_of_incomplete_line.take() {
            if can_append_to_line {
                can_append_to_line = false;
                let last_line = self.rx.last_mut().expect("can't append to nothing");
                let last_index = last_line.range().start;
                // assert_eq!(last_index, index);

                // let start = range_slice.range.start;
                let trunc = last_index..index_in_buffer + trunc.len();
                let trunc = raw_buffer
                    .range(trunc)
                    .expect("failed to get truncated line-to-continue buffer");
                let orig = last_index..index_in_buffer + orig.len();
                let orig = raw_buffer
                    .range(orig)
                    .expect("failed to get line-to-continue buffer");

                let kit_for_append = BufLineKit {
                    full_range_slice: unsafe {
                        RangeSlice::from_parent_and_child(&raw_buffer.inner, orig)
                    },
                    ..kit
                };

                let buf_line_opt = slice_as_port_text(kit_for_append, color_rules, line_ending);

                if let Some(line) = buf_line_opt {
                    last_line.update_line(line);
                } else {
                    _ = self.rx.pop();
                    self.last_rx_was_complete = true;
                    // last_line.clear_line();
                }
                self.last_rx_was_complete = orig.has_line_ending(line_ending);
                continue;
            }
            // Otherwise, new line being created.
            let kit_for_new = BufLineKit {
                full_range_slice: unsafe {
                    RangeSlice::from_parent_and_child(&raw_buffer.inner, orig)
                },
                ..kit
            };

            // Returns None if the slice would be fully hidden by the color rules.
            fn slice_as_port_text(
                kit: BufLineKit,
                color_rules: &ColorRules,
                line_ending: &LineEnding,
            ) -> Option<BufLine> {
                let lossy_flavor = if kit.render.rendering.escape_unprintable_bytes {
                    LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
                } else {
                    LossyFlavor::replacement_char()
                };

                let RangeSlice {
                    range,
                    slice: original,
                } = &kit.full_range_slice;

                let truncated = if original.has_line_ending(line_ending) {
                    &original[..original.len() - line_ending.as_bytes().len()]
                } else {
                    original
                };

                let line = match truncated.to_line_lossy(Style::new(), lossy_flavor) {
                    Ok(line) => line,
                    Err(_) => {
                        error!("ansi-to-tui failed to parse input! Using unstyled text.");
                        Line::from(String::from_utf8_lossy(truncated).to_string())
                    }
                };

                color_rules
                    .apply_onto(truncated, line, lossy_flavor)
                    .map(|mut l| {
                        l.remove_unsavory_chars(kit.render.rendering.escape_unprintable_bytes);
                        let line: Line<'static> = l.new_owned();
                        BufLine::port_text_line(line, kit, line_ending)
                    })
            }

            if let Some(new_bufline) = slice_as_port_text(kit_for_new, color_rules, line_ending) {
                self.rx.push(new_bufline);
            }
            self.last_rx_was_complete = orig.has_line_ending(line_ending);
        }
    }
    #[cfg(feature = "defmt")]
    fn consume_frame(
        &mut self,
        kit: BufLineKit,
        decoder: &DefmtDecoder,
        frame: &defmt_decoder::Frame<'_>,
        color_rules: &ColorRules,
    ) {
        // choosing to skip pushing instead of filtering
        // to keep consistent with other logic that expects
        // invisible lines to not be pushed,
        // and to skip further handling for those lines.
        let commit_frame = match frame.level() {
            None => true,
            Some(level) => kit.render.defmt.max_log_level <= crate::settings::Level::from(level),
        };
        if !commit_frame {
            return;
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

            if let Some(mut line) = color_rules.apply_onto(
                line.as_bytes(),
                message_line,
                LossyFlavor::ReplacementChar(None),
            ) {
                line.remove_unsavory_chars(kit.render.rendering.escape_unprintable_bytes);
                let owned_line = line.new_owned();

                let kit = BufLineKit {
                    full_range_slice: kit.full_range_slice.clone(),
                    ..kit
                };

                self.rx.push(BufLine::port_defmt_line(
                    owned_line,
                    kit,
                    frame.level(),
                    device_timestamp_ref,
                    loc_opt.clone(),
                ));
            }
        }
    }
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

    #[cfg(feature = "logging")]
    pub log_handle: LoggingHandle,
    #[cfg(feature = "logging")]
    log_thread: Takeable<JoinHandle<()>>,
    #[cfg(feature = "logging")]
    log_settings: Logging,
    #[cfg(feature = "defmt")]
    pub defmt_decoder: Option<Arc<DefmtDecoder>>,
    #[cfg(feature = "defmt")]
    defmt_settings: Defmt,
    #[cfg(feature = "defmt")]
    defmt_raw_malformed: bool,
    // #[cfg(feature = "defmt")]
    // frame_delimiter: FrameDelimiter,
}

#[cfg(feature = "logging")]
impl Drop for Buffer {
    fn drop(&mut self) {
        debug!("Shutting down Logging worker");
        if self.log_handle.shutdown().is_ok() {
            let log_thread = self.log_thread.take();

            if log_thread.join().is_err() {
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
    // TODO lower sources of truth for all these settings structs.
    // Rc<Something> with the settings that's shared around?
    pub fn build(
        line_ending: &[u8],
        rendering: Rendering,
        #[cfg(feature = "logging")] logging: Logging,
        #[cfg(feature = "logging")] event_tx: Sender<Event>,
        #[cfg(feature = "defmt")] defmt: Defmt,
    ) -> Result<Self, ColorRuleError> {
        let line_ending: LineEnding = line_ending.into();
        #[cfg(feature = "logging")]
        let (log_handle, log_thread) = LoggingHandle::new(
            line_ending.clone(),
            logging.clone(),
            event_tx,
            #[cfg(feature = "defmt")]
            defmt.clone(),
        );

        let color_rules = ColorRules::load_from_file(config_adjacent_path(COLOR_RULES_PATH))?;

        Ok(Self {
            raw: RawBuffer {
                inner: Vec::with_capacity(1024),
                buffer_timestamps: Vec::with_capacity(1024),
                consumed_up_to: 0,
            },
            styled_lines: StyledLines {
                rx: Vec::with_capacity(1024),
                last_rx_was_complete: true,
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
            color_rules,
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
            #[cfg(feature = "defmt")]
            defmt_raw_malformed: false,
        })
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
                .map(|b| format!("\\x{b:02X}"))
                .join("")
                .into();

            text.dark_gray().italic().bold()
        } else {
            span!(Style::new().dark_gray(); "*".repeat(bytes.len()))
        };

        let line = Line::from(vec![user_span, text]);

        let combined: Vec<_> = bytes.iter().chain(line_ending.iter()).copied().collect();
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
        let kit = BufLineKit {
            timestamp: now,
            area_width: self.last_terminal_size.width,
            render: self.line_render_settings(),
            full_range_slice: RangeSlice {
                range: self.raw.inner.len()..self.raw.inner.len(),
                slice: &[],
            },
        };
        let user_buf_line = BufLine::user_line(line, kit, &line_ending, true, is_macro, &combined);

        self.styled_lines.last_rx_was_complete = self
            .rendering
            .echo_user_input
            .filter_user_line(&user_buf_line.line_type)
            || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));

        #[cfg(feature = "logging")]
        if self.log_settings.log_text_to_file && self.log_settings.log_user_input {
            self.log_handle
                .log_tx_bytes(now, bytes.to_owned(), line_ending.as_bytes().to_owned())
                .expect("Logging worker has disappeared!");
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
                Line::from(vec![user_span.clone(), text])
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
            let kit = BufLineKit {
                timestamp: now,
                area_width: self.last_terminal_size.width,
                render: self.line_render_settings(),
                full_range_slice: RangeSlice {
                    range: self.raw.inner.len()..self.raw.inner.len(),
                    slice: &[],
                },
            };
            let user_buf_line = BufLine::user_line(line, kit, &line_ending, false, is_macro, orig);
            // Used to be out of the for loop.
            self.styled_lines.last_rx_was_complete = self
                .rendering
                .echo_user_input
                .filter_user_line(&user_buf_line.line_type)
                || (self.raw.inner.is_empty() || self.raw.inner.has_line_ending(&self.line_ending));

            #[cfg(feature = "logging")]
            if self.log_settings.log_text_to_file && self.log_settings.log_user_input {
                self.log_handle
                    .log_tx_bytes(
                        now,
                        text.as_bytes().to_owned(),
                        line_ending.as_bytes().to_owned(),
                    )
                    .expect("Logging worker has disappeared!");
            }
            self.styled_lines.tx.push(user_buf_line);
        }
    }

    // Forced to use Vec<u8> for now
    pub fn fresh_rx_bytes(&mut self, bytes: Vec<u8>) {
        let now = Local::now();
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);

        self.raw.feed(&bytes, now);

        #[cfg(feature = "logging")]
        self.log_handle.log_rx_bytes(now, bytes).unwrap();

        // let meow = std::time::Instant::now();
        self.consume_latest_bytes(now);
        // error!("{:?}", meow.elapsed());

        // self.raw.inner.extend(bytes.iter());
        // while self.consume_latest_bytes(Some(now)) {
        //     ();
        // }
    }

    fn consume_latest_bytes(&mut self, timestamp: DateTime<Local>) {
        #[cfg(not(feature = "defmt"))]
        while let Some((index_in_buffer, delimited_slice)) = self.raw.next_slice_raw() {
            let DelimitedSlice::Raw(slice) = delimited_slice else {
                unreachable!();
            };
            let kit = BufLineKit {
                timestamp,
                area_width: self.last_terminal_size.width,
                render: RenderSettings {
                    rendering: &self.rendering,
                    #[cfg(feature = "defmt")]
                    defmt: &self.defmt_settings,
                },
                full_range_slice: unsafe {
                    RangeSlice::from_parent_and_child(&self.raw.inner, slice)
                },
            };

            self.styled_lines.consume_as_text(
                &self.raw,
                &self.color_rules,
                index_in_buffer,
                delimited_slice,
                kit,
                &self.line_ending,
            );
            self.raw.consumed(slice.len());
        }

        #[cfg(feature = "defmt")]
        while let Some((index_in_buffer, delimited_slice)) = self
            .raw
            .next_slice(self.defmt_settings.defmt_parsing.clone())
            && !self.defmt_raw_malformed
        {
            match delimited_slice {
                DelimitedSlice::Raw(slice) => {
                    let kit = BufLineKit {
                        timestamp,
                        area_width: self.last_terminal_size.width,
                        render: RenderSettings {
                            rendering: &self.rendering,
                            defmt: &self.defmt_settings,
                        },
                        full_range_slice: unsafe {
                            RangeSlice::from_parent_and_child(&self.raw.inner, slice)
                        },
                    };

                    self.styled_lines.consume_as_text(
                        &self.raw,
                        &self.color_rules,
                        index_in_buffer,
                        delimited_slice,
                        kit,
                        &self.line_ending,
                    );
                    self.raw.consumed(slice.len());
                }
                DelimitedSlice::DefmtRaw(raw_uncompressed) => {
                    if let Some(decoder) = &self.defmt_decoder {
                        let kit = BufLineKit {
                            timestamp,
                            area_width: self.last_terminal_size.width,
                            render: RenderSettings {
                                rendering: &self.rendering,
                                defmt: &self.defmt_settings,
                            },
                            full_range_slice: unsafe {
                                RangeSlice::from_parent_and_child(&self.raw.inner, raw_uncompressed)
                            },
                        };

                        match decoder.table.decode(raw_uncompressed) {
                            Ok((frame, consumed)) => {
                                self.styled_lines.consume_frame(
                                    kit,
                                    decoder,
                                    &frame,
                                    &self.color_rules,
                                );
                                self.styled_lines.last_rx_was_complete = true;

                                self.raw.consumed(consumed);
                            }
                            Err(defmt_decoder::DecodeError::UnexpectedEof) => {
                                break;
                            }
                            Err(defmt_decoder::DecodeError::Malformed) => {
                                self.defmt_raw_malformed = true;
                                let line =
                                    Line::raw("defmt raw parse error, ceasing further attempts.");
                                self.styled_lines.rx.push(BufLine::port_text_line(
                                    line,
                                    kit,
                                    &LineEnding::None,
                                ));
                                break;
                            }
                        }
                    }
                }
                DelimitedSlice::DefmtRzcobs { raw, inner } => {
                    let kit = BufLineKit {
                        timestamp,
                        area_width: self.last_terminal_size.width,
                        render: RenderSettings {
                            rendering: &self.rendering,
                            defmt: &self.defmt_settings,
                        },
                        full_range_slice: unsafe {
                            RangeSlice::from_parent_and_child(&self.raw.inner, raw)
                        },
                    };
                    let raw_slice_len = raw.len();

                    if let Some(decoder) = &self.defmt_decoder {
                        if let Ok(uncompressed) = rzcobs_decode(inner) {
                            if let Ok((frame, _consumed)) = decoder.table.decode(&uncompressed) {
                                self.styled_lines.consume_frame(
                                    kit,
                                    decoder,
                                    &frame,
                                    &self.color_rules,
                                );
                            } else {
                                self.styled_lines.failed_decode(
                                    delimited_slice,
                                    DefmtPacketError::DefmtDecode,
                                    kit,
                                    &self.line_ending,
                                );
                            }
                        } else {
                            self.styled_lines.failed_decode(
                                delimited_slice,
                                DefmtPacketError::RzcobsDecompress,
                                kit,
                                &self.line_ending,
                            );
                        }
                    } else {
                        self.styled_lines.failed_decode(
                            delimited_slice,
                            DefmtPacketError::NoDecoder,
                            kit,
                            &self.line_ending,
                        );
                    }

                    self.styled_lines.last_rx_was_complete = true;
                    self.raw.consumed(raw_slice_len);
                }
            }
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
                    b.range().start,
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
        let user_echo = self.rendering.echo_user_input;

        // No lines to append to.
        self.styled_lines.last_rx_was_complete = true;

        #[cfg(feature = "defmt")]
        {
            self.defmt_raw_malformed = false;
        }

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
                self.styled_lines.last_rx_was_complete = true;
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
                lines[0].range().start < lines[1].range().start,
                "Port lines should be in exact ascending order by index."
            );
            assert_eq!(
                lines[0].range().end,
                lines[1].range().start,
                "Port line slice should end where next one begins.",
            );
        });

        // Returning variables we stole back to self.
        // (excluding the raw buffer since that got reconsumed gradually back into self)
        // self.raw.buffer_timestamps = rx_timestamps;
        // self.styled_lines.tx = user_lines;
        self.scroll_by(0);
    }

    #[cfg(feature = "logging")]
    /// Relog contents in buffer
    pub fn relog_buffer(&mut self) -> HandleResult<()> {
        use crossbeam::channel::unbounded;

        if self.raw.inner.is_empty() {
            warn!("Can't relog an empty buffer!");
            return Ok(());
        }

        // Taking these variables out of `self` temporarily to allow running &mut self methods while holding
        // references to these.
        let user_timestamps: Vec<_> = self
            .styled_lines
            .tx
            .iter()
            .filter(|s| self.log_settings.log_user_input)
            .map(|b| {
                let LineType::User { .. } = &b.line_type else {
                    unreachable!();
                };

                (b.line_type.clone(), b.range().start, b.timestamp)
            })
            .collect();
        let orig_buf_len = self.raw.inner.len();
        let timestamps_len = self.raw.buffer_timestamps.len();
        let user_echo = self.rendering.echo_user_input;

        // No lines to append to.
        self.styled_lines.last_rx_was_complete = true;

        #[cfg(feature = "defmt")]
        {
            self.defmt_raw_malformed = false;
        }

        let blank_port_line_type = LineType::Port {
            escaped_line_ending: None,
        };
        // Getting all time-tagged indices in the buffer where either
        // 1. Data came in through the port
        // 2. The user sent data
        let interleaved_points = interleave_by(
            self.raw
                .buffer_timestamps
                .iter()
                .map(|(index, timestamp)| (*index, *timestamp, blank_port_line_type.clone()))
                // Add a "finale" element to capture any remaining buffer, always placed at the end.
                .chain(std::iter::once((
                    orig_buf_len,
                    Local::now(),
                    blank_port_line_type.clone(),
                ))),
            user_timestamps
                .into_iter()
                // If a user line isn't visible, ignore it when taking external new-lines into account.
                .map(|(line_type, index, timestamp)| (index, timestamp, line_type)),
            // Interleaving by sorting in order of raw_buffer_index, if they're equal, then whichever has a sooner timestamp.
            |port, user| match port.0.cmp(&user.0) {
                Ordering::Equal => port.1 <= user.1,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        );

        debug!("total len: {orig_buf_len}");

        let buffer_slices = interleaved_points
            .tuple_windows()
            // Filtering out some empty slices, unless they indicate a user event.
            .filter(|((start_index, _, line_type), (end_index, _, _))| {
                start_index != end_index || matches!(line_type, LineType::User { .. })
            })
            // Building the parent slices (pre-newline splitting)
            .map(|((start_index, timestamp, line_type), (end_index, _, _))| {
                (
                    &self.raw.inner[start_index..end_index],
                    timestamp,
                    line_type,
                    (start_index, end_index),
                )
            });

        let (relog_tx, relog_rx) = unbounded();
        use crate::buffer::logging::{Relogging, TxPayload};

        self.log_handle.begin_relogging(relog_rx)?;

        let mut rx_batch = Vec::new();
        let mut tx_batch = Vec::new();

        for (slice, timestamp, line_type, (slice_start, slice_end)) in buffer_slices {
            // If this was where a user line we allow to render is,
            // then we'll finish this line early if it's not already finished.
            if let LineType::User {
                is_bytes,
                is_macro,
                escaped_line_ending,
                reloggable_raw,
            } = line_type
            {
                if !rx_batch.is_empty() {
                    let transmitted_rx = std::mem::take(&mut rx_batch);
                    relog_tx.send(Relogging::RxBatch(transmitted_rx))?;
                }

                let line_ending_bytes =
                    escaped_line_ending.map_or(Vec::new(), |le| le.as_bytes().to_owned());

                tx_batch.push(TxPayload {
                    timestamp,
                    bytes: reloggable_raw,
                    line_ending: line_ending_bytes,
                });

                continue;
            } else {
                if !tx_batch.is_empty() {
                    let transmitted_tx = std::mem::take(&mut tx_batch);
                    relog_tx.send(Relogging::TxBatch(transmitted_tx))?;
                }

                rx_batch.push((timestamp, slice.to_owned()));
            }
        }

        if !rx_batch.is_empty() {
            relog_tx.send(Relogging::RxBatch(rx_batch))?;
        } else if !tx_batch.is_empty() {
            relog_tx.send(Relogging::TxBatch(tx_batch))?;
        }

        relog_tx.send(Relogging::Done)?;

        Ok(())
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
        }
    }
    pub fn update_render_settings(&mut self, rendering: Rendering) {
        let old = std::mem::replace(&mut self.rendering, rendering);
        let new = &self.rendering;
        let should_reconsume = changed!(old, new, echo_user_input, escape_unprintable_bytes);

        let should_rewrap_lines = changed!(
            old,
            new,
            timestamps,
            show_indices,
            indices_as_hex,
            show_line_ending
        );

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

        #[cfg(all(feature = "logging", feature = "defmt"))]
        self.log_handle
            .update_defmt_settings(self.defmt_settings.clone())
            .unwrap();

        self.scroll_by(0);
    }

    #[cfg(feature = "logging")]
    pub fn update_logging_settings(&mut self, logging: Logging) {
        self.log_handle.update_settings(logging).unwrap();
    }
    pub fn intentional_disconnect_clear(&mut self) {
        #[cfg(feature = "logging")]
        self.log_handle.log_port_disconnected(true).unwrap();

        self.styled_lines.rx.clear();
        self.styled_lines.rx.shrink_to(1024);

        self.styled_lines.tx.clear();
        self.styled_lines.tx.shrink_to(1024);

        self.styled_lines.last_rx_was_complete = true;

        self.raw.reset();
    }
    // pub fn render_settings(&self) -> RenderSettings {
    //     RenderSettings {
    //         rendering: &self.rendering,
    //         #[cfg(feature = "defmt")]
    //         defmt: &self.defmt_settings,
    //     }
    // }
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
    match line_ending {
        LineEnding::None => Either::Left(std::iter::once((bytes, bytes, (0, bytes.len())))),
        line_ending => Either::Right(_line_ending_iter(bytes, line_ending)),
    }
}

// Internal impl, for actually iterating through line endings.
#[inline(always)]
fn _line_ending_iter<'a>(
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

    let line_ending_pos_iter = line_ending_pos_iter
        .into_iter()
        .map(|line_ending_index| (line_ending_index, false))
        .chain(std::iter::once((bytes.len(), true)));

    let mut last_index = 0;

    // iter of continuous slices, non-overlapping
    line_ending_pos_iter.filter_map(move |(line_ending_index, is_final_entry)| {
        let result = if is_final_entry && last_index == bytes.len() && !bytes.is_empty() {
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
    })
}
