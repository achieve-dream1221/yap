#[cfg(feature = "defmt")]
use std::sync::Arc;
use std::{cell::Cell, cmp::Ordering, ops::Range};

use ansi_to_tui::{IntoText, LossyFlavor};
use bstr::{ByteSlice, ByteVec};
use buf_line::{BufLine, LineType};
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString};

use itertools::{Either, Itertools};
use memchr::memmem::Finder;
use ratatui::{
    layout::Size,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::ScrollbarState,
};
use ratatui_macros::span;
use tracing::{debug, error, warn};

use crate::{
    buffer::buf_line::{BufLineKit, LineFinished, RenderSettings},
    changed,
    settings::{Rendering, Settings},
    traits::{ByteSuffixCheck, LineHelpers, interleave_by},
    tui::color_rules::ColorRules,
};

#[cfg(feature = "defmt")]
use crate::{
    buffer::{
        buf_line::FrameLocation,
        defmt::{DefmtDecoder, DefmtPacketError, rzcobs_decode},
    },
    settings::{Defmt, DefmtSupport},
};

#[cfg(feature = "logging")]
use crate::{app::Event, settings::Logging};

mod buf_line;
mod hex_spans;
pub use hex_spans::*;
mod range_slice;
pub use range_slice::RangeSlice;
mod tui;

#[cfg(feature = "defmt")]
pub mod defmt;

#[cfg(feature = "logging")]
mod logging;

#[cfg(feature = "logging")]
pub use logging::{DEFAULT_TIMESTAMP_FORMAT, LoggingEvent, LoggingHandle, LoggingWorkerMissing};
#[cfg(feature = "logging")]
use {crossbeam::channel::Sender, takeable::Takeable};

#[cfg(test)]
mod tests;

pub struct Buffer {
    /// Raw bytes from the port.
    raw: RawBuffer,
    /// Post-processed
    /// (UTF-8 -> ANSI -> Color Rules)
    /// ratatui Lines to be rendered.
    styled_lines: StyledLines,
    /// Cache for `combined_height`'s output
    cached_combined_height: Cell<Option<usize>>,

    /// The last known size of the area given to
    /// render the buffer in (including the area taken by the scrollbar.)
    last_terminal_size: Size,

    pub state: BufferState,

    /// Clone of Rendering settings, ideally should be in an Rc or something
    /// similar to ArcSwap so that I can change it under it's nose.
    rendering: Rendering,

    /// Current RX line ending.
    line_ending: LineEnding,

    /// Text coloring, censoring, and omitting rules.
    color_rules: ColorRules,

    #[cfg(feature = "logging")]
    pub log_handle: LoggingHandle,
    #[cfg(feature = "logging")]
    log_thread: Takeable<std::thread::JoinHandle<()>>,
    #[cfg(feature = "logging")]
    /// Clone of Logging settings, ditto the sentiment from Rendering.
    log_settings: Logging,

    #[cfg(feature = "defmt")]
    /// Populated when a defmt ELF is successfully loaded.
    pub defmt_decoder: Option<Arc<DefmtDecoder>>,
    #[cfg(feature = "defmt")]
    /// Clone of Defmt settings, ditto.
    defmt_settings: Defmt,
    #[cfg(feature = "defmt")]
    /// Flag if last defmt raw/uncompressed decode attempt failed.
    /// Further parsing attempts will not be allowed if set to true.
    defmt_raw_malformed: bool,
}

#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
/// Controls for how the buffer will be rendered.
pub struct BufferState {
    /// Positive offset from bottom, scrolling up into past inputs.
    vert_scroll: usize,
    scrollbar_state: ScrollbarState,
    /// Keep scroll stuck to bottom, on the newest inputs.
    // TODO maybe remove and use with vert_scroll == 0?
    stuck_to_bottom: bool,
    /// When in Hex View, controls how many bytes are shown per row,
    /// filled by `determine_bytes_per_line`, optionally capped by user.
    hex_bytes_per_line: u8,
    // Calculated width of area to render bytes in, also filled by `determine_bytes_per_line`.
    hex_section_width: u16,
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
/// Whether User input entered via the Pseudo-shell/Macro invocations should be displayed,
///
/// Also affects whether a user's input would interrupt an incomplete port line,
/// making it start fresh in a new line under the user's.
pub enum UserEcho {
    #[strum(serialize = "false")]
    /// No user input should be shown.
    None,
    #[strum(serialize = "true")]
    // All user input should be shown.
    All,
    // #[strum(serialize = "All but No Macros")]
    /// Only user-typed input should be shown.
    #[cfg(feature = "macros")]
    NoMacros,
    // #[strum(serialize = "All but No Bytes")]
    /// All user _text_ input should be shown, pseudo-shell's byte mode entries are omitted,
    /// and any macros that contain escaped byte sequences (i.e. `\n` or `\xFF`)
    NoBytes,
    /// Only user-typed _text_ will be shown.
    #[cfg(feature = "macros")]
    NoMacrosOrBytes,
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
            #[cfg(feature = "macros")]
            UserEcho::NoMacros => !line_type.is_macro(),
            #[cfg(feature = "macros")]
            UserEcho::NoMacrosOrBytes => !line_type.is_bytes() && !line_type.is_macro(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
/// Represents a line ending sequence used to split incoming bytes into lines.
pub enum LineEnding {
    /// No line ending! Uninterrupted input is treated as one continuous line.
    None,
    /// A single byte (e.g. `\n`)
    Byte(u8),
    /// Multi-byte line ending, using a precompiled memem Finder (e.g. `\r\n`)
    MultiByte(Finder<'static>),
}

impl LineEnding {
    /// Return the needle the LineEnding represents.
    fn as_bytes(&self) -> &[u8] {
        match self {
            LineEnding::None => &[],
            LineEnding::Byte(byte) => std::slice::from_ref(byte),
            LineEnding::MultiByte(finder) => finder.needle(),
        }
    }

    /// Return a printable escaped version of the line ending.
    fn as_escaped(&self) -> Option<CompactString> {
        match self {
            LineEnding::None => None,
            _ => Some(self.as_bytes().escape_bytes().to_compact_string()),
        }
    }

    /// If the supplied slice ends with this LineEnding, return the escaped representation of it.
    fn escaped_from(&self, buffer: &[u8]) -> Option<CompactString> {
        if buffer.has_line_ending(self) {
            self.as_escaped()
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

#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
struct RawBuffer {
    /// Raw bytes as recieved from the serial port.
    inner: Vec<u8>,
    /// Time-tagged indexes into `raw_buffer`, from each input from the port.
    buffer_timestamps: Vec<(usize, DateTime<Local>, usize)>,
    /// Slice retrieval methods start from this index.
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
    /// Fresh bytes!
    fn feed(&mut self, new: &[u8], timestamp: DateTime<Local>) {
        // warn!("fed {} bytes", new.len());
        self.buffer_timestamps
            .push((self.inner.len(), timestamp, new.len()));
        self.inner.extend(new);
    }
    fn consumed(&mut self, amount: usize) {
        self.consumed_up_to += amount;
    }
    /// Returns all unconsumed bytes as-is.
    ///
    /// Returns None if no new bytes are ready.
    fn next_slice_raw(&self) -> Option<(usize, DelimitedSlice<'_>)> {
        let start_index = self.consumed_up_to;
        let newest = &self.inner[start_index..];
        if newest.is_empty() {
            None
        } else {
            Some((start_index, DelimitedSlice::Unknown(newest)))
        }
    }
    /// Returns the next tagged chunk of bytes to be processed.
    ///
    /// Returns None if either no new bytes are ready or rzcobs sequence is unterminated.
    #[cfg(feature = "defmt")]
    fn next_slice_checked(
        &self,
        defmt_support: DefmtSupport,
    ) -> Option<(usize, DelimitedSlice<'_>)> {
        match defmt_support {
            DefmtSupport::FramedRzcobs => self.next_slice_defmt_rzcobs(true),
            DefmtSupport::UnframedRzcobs => self.next_slice_defmt_rzcobs(false),
            DefmtSupport::Raw => self.next_slice_defmt_raw(),
            DefmtSupport::Disabled => self.next_slice_raw(),
        }
    }
    #[cfg(feature = "defmt")]
    /// Returns all unconsumed bytes as uncompressed defmt frame data.
    ///
    /// Returns None if no new bytes are ready.
    fn next_slice_defmt_raw(&self) -> Option<(usize, DelimitedSlice<'_>)> {
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
    /// If `esp_println_framed` is `true`, looks for defmt frames framed by: 0xFF 0x00 ... 0x00, returning None if not found.
    ///
    /// (this is to differenciate defmt frames from plaintext, which both appear in the bytestream for ESP32s)
    ///
    /// Otherwise, looks for a subslice ending with 0x00, returning None if not found.
    ///
    /// Returns None if either a defmt frame is incomplete, or there is no new data to give.
    fn next_slice_defmt_rzcobs(
        &self,
        esp_println_framed: bool,
    ) -> Option<(usize, DelimitedSlice<'_>)> {
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
/// The type returned by RawBuffer when requesting fresh slices.
pub enum DelimitedSlice<'a> {
    #[cfg(feature = "defmt")]
    /// Used by both _framed_ rzCOBS-encoded inputs, such as from esp-rs/esp-println,
    /// and _plain_ rzCOBS (and thus implicity zero-delimited) input.
    DefmtRzcobs {
        /// Complete original slice, containing prefix and terminator.
        raw: &'a [u8],
        /// (Supposedly) rzCOBS packet, stripped of any delimiters.
        inner: &'a [u8],
    },
    #[cfg(feature = "defmt")]
    /// Use if ELF has raw encoding enabled (no rzCOBS compression).
    DefmtRaw(&'a [u8]),
    /// Non-defmt input, either junk data or plain ASCII/UTF-8 logs.
    Unknown(&'a [u8]),
}

#[cfg(feature = "defmt")]
impl DelimitedSlice<'_> {
    pub fn raw_len(&self) -> usize {
        match self {
            #[cfg(feature = "defmt")]
            DelimitedSlice::DefmtRzcobs { raw, .. } => raw.len(),
            #[cfg(feature = "defmt")]
            DelimitedSlice::DefmtRaw(raw) => raw.len(),
            DelimitedSlice::Unknown(raw) => raw.len(),
        }
    }
}

#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
struct StyledLines {
    /// Text and Defmt lines recieved from the port.
    rx: Vec<BufLine>,
    /// User-sent psuedo-shell inputs and macros.
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
        let raw = match delimited_slice {
            DelimitedSlice::DefmtRaw(raw) => raw,
            DelimitedSlice::DefmtRzcobs { raw, .. } => raw,
            _ => unreachable!("non-defmt slice can't fail decoding"),
        };

        let mut text = format!("Couldn't decode defmt rzcobs packet ({reason}): ");
        text.extend(raw.iter().map(|b| format!("{b:02X}")));

        self.rx.push(BufLine::port_text_line(
            Line::raw(text),
            kit,
            None,
            line_ending,
        ));
    }
    /// Separate newly recieved byteslice by given line ending,
    /// then parse and add to StyledLines.
    fn consume_as_text(
        &mut self,
        raw_buffer: &RawBuffer,
        color_rules: &ColorRules,
        kit: BufLineKit,
        line_ending: &LineEnding,
    ) {
        // Check if the last line was unfinished, and also grab an optionally-present record
        // it may have of an ANSI Clear Line previously being encountered.
        let mut continue_for_first_line: Option<Option<(usize, Style)>> =
            self.rx
                .last()
                .and_then(|b| match b.line_type.line_finished() {
                    Some(LineFinished::Unfinished { clear_occurred }) => Some(*clear_occurred),
                    _ => None,
                });

        // If the last line was unfinished, then the next result
        let mut first_should_replace = continue_for_first_line.is_some();

        // If last line was unfinished, expand the slice-to-consume to also
        // contain the very beginning of the line's spot in the buffer.
        //
        // Much later TODO: This is technically the source of a specific bottleneck.
        // If a suuuuuper long line (kilobytes+) keeps being added to, then each time
        // new bytes come in, then we need to reconsume the _whole_ line's content in order
        // to ensure Color Rules get applied properly.
        // For example, if you have a rule matching "abcd" to color it Red,
        // and you first recieve only "abc" and then "d", it's nontrivial to go back and
        // find "abcd" in the previous pre-rendered line across several Spans
        // (that are no longer contiguous in memory! as each Span owns their text)
        // and recolor the rule's matched characters.
        //
        // It would be easier to detect if the hash of all coloring actions has changed and if so,
        // act accordingly, potentially flattening actions on top of each other and comparing to
        // find the first discrepency, and replacing only spans from that point.
        //
        // Out of scope for an initial public v0.1.0 release though.
        let slice = if first_should_replace {
            let last_line = self.rx.last_mut().expect("can't replace nothing");

            let last_line_start = last_line.range().start;
            let slice_finish = kit.full_range_slice.range.end;

            // New bytes + start of last line's unfinished buffer
            &raw_buffer.inner[last_line_start..slice_finish]
        } else {
            // Otherwise, just the bytes we were starting with.
            kit.full_range_slice.slice
        };

        // Break slice apart by line ending.
        let lines = line_ending_iter(slice, line_ending).map(|(_trunc, orig, _range)| {
            let kit = BufLineKit {
                full_range_slice: unsafe {
                    RangeSlice::from_parent_and_child(&raw_buffer.inner, orig)
                },
                ..kit
            };

            // Convert bytes to text and perform color rule operations
            Self::slice_as_port_text(
                kit,
                // .take() ensures it only happens for the first emitted line
                continue_for_first_line.take().flatten(),
                color_rules,
                line_ending,
            )
        });

        for line in lines {
            if first_should_replace {
                first_should_replace = false;

                let last_line = self.rx.last_mut().expect("can't replace nothing");
                last_line.replace_contents_with(line);
            } else {
                self.rx.push(line);
            }
        }
    }
    /// Parse bytes as a single line from the connected device
    /// (line-ending separation should be handled by caller).
    ///
    /// Converts given bytes to UTF-8, replacing invalid sequences,
    /// applying inline ANSI styles, applying user-defined ColorRules,
    /// and removing "unsavory" characters that mess with ratatui's rendering.
    fn slice_as_port_text(
        kit: BufLineKit,
        continue_from: Option<(usize, Style)>,
        color_rules: &ColorRules,
        line_ending: &LineEnding,
    ) -> BufLine {
        debug_assert!(!kit.full_range_slice.slice.is_empty());

        let lossy_flavor = if kit.render.rendering.escape_unprintable_bytes {
            LossyFlavor::escaped_bytes_styled(Style::new().dark_gray())
        } else {
            LossyFlavor::replacement_char()
        };

        let RangeSlice {
            slice: original, ..
        } = &kit.full_range_slice;

        if *original == line_ending.as_bytes() {
            // Return hollow line (properly terminated but with no internal content).
            return BufLine::hollow_port_line(kit, line_ending);
        }

        // Strip line ending from the buffer to parse if present, we just care about
        // the internal text in question.
        let truncated = if original.has_line_ending(line_ending) {
            &original[..original.len() - line_ending.as_bytes().len()]
        } else {
            original
        };

        // If we encountered an ANSI Clear Line command in a previous scan through this buffer,
        // we don't need to reconsume those cleared bytes utf-8 or ansi-to-tui conversion!
        //
        // We do however still need the whole buffer for color rule determination though,
        // since it's still part of the same line (just not yet terminated).
        //
        // If None, starts from 0 with a default style.
        let (continue_index, continue_style) = continue_from.unwrap_or_default();
        let truncated_continued = &truncated[continue_index..];

        let (line, clear_info) =
            match truncated_continued.to_line_lossy_flagged(continue_style, lossy_flavor) {
                Ok((line, new_clear_info)) => {
                    let combined_clear_info = if let Some((new_index, style)) = new_clear_info {
                        // Add together indices, since new one is relative to continue_index.
                        Some((new_index + continue_index, style))
                    } else {
                        continue_from
                    };
                    (line, combined_clear_info)
                }
                Err(e) => {
                    // I think this is technically unreachable
                    // Since ansi-to-tui errors on invalid UTF-8 (not possible in this case),
                    // and on a nom error, which I don't think is possible, even on empty/incomplete sequences.
                    // But I'd rather handle the almost-impossible case than just crash.
                    error!("ansi-to-tui failed to parse input! {e} Using unstyled text.");
                    (
                        Line::from(String::from_utf8_lossy(truncated_continued).to_string()),
                        None,
                    )
                }
            };

        if let Some(mut recolored_line) = color_rules.apply_onto(truncated, line) {
            recolored_line.remove_unsavory_chars(kit.render.rendering.escape_unprintable_bytes);
            let line: Line<'static> = recolored_line.new_owned();
            BufLine::port_text_line(line, kit, clear_info, line_ending)
        } else {
            BufLine::hidden_content_port_line(kit, line_ending)
        }
    }
    #[cfg(feature = "defmt")]
    /// A valid defmt frame has been parsed, convert into a single BufLine,
    /// running through the same Color Rules.
    fn consume_frame(
        &mut self,
        kit: BufLineKit,
        decoder: &DefmtDecoder,
        frame: &defmt_decoder::Frame<'_>,
        color_rules: &ColorRules,
    ) {
        // let meow = std::time::Instant::now();
        // error!("{:?}", meow.elapsed());
        // debug!("{frame:#?}");

        // Get location of log invocation in source file
        let loc_opt = decoder
            .locations
            .as_ref()
            .and_then(|locs| locs.get(&frame.index()))
            .map(FrameLocation::from);

        // Get just the log's text content,
        // timestamp and level are handled separately.
        let message = frame.display_message().to_string();
        // Break into lines if more than one is present
        let message_lines = message.lines();

        let device_timestamp = frame.display_timestamp();
        let device_timestamp_ref = device_timestamp
            .as_ref()
            // display_timestamp returns an object whose only purpose
            // is as a fmt::Display adapter, so just use it as that.
            .map(|ts| ts as &dyn std::fmt::Display);

        for line in message_lines {
            // Make a line like how ansi-to-tui would if given a
            // byteslice with no ANSI styling, all in one Span.
            let mut message_line = Line::default();
            message_line.push_span(Span::raw(line));

            let kit = BufLineKit {
                full_range_slice: kit.full_range_slice.clone(),
                ..kit
            };
            // And apply the same color rules, using the post-defmt-expanded text as the backing parent buffer.
            if let Some(mut line) = color_rules.apply_onto(line.as_bytes(), message_line) {
                line.remove_unsavory_chars(kit.render.rendering.escape_unprintable_bytes);
                let owned_line = line.new_owned();

                self.rx.push(BufLine::port_defmt_line(
                    owned_line,
                    kit,
                    frame.level(),
                    device_timestamp_ref,
                    loc_opt.clone(),
                ));
            } else {
                self.rx.push(BufLine::hidden_content_port_defmt_line(
                    kit,
                    frame.level(),
                    device_timestamp_ref,
                    loc_opt.clone(),
                ));
            }
        }
    }
}

impl Buffer {
    // TODO lower sources of truth for all these settings structs.
    // Rc<Something> with the settings that's shared around?
    pub fn new(
        line_ending: &[u8],
        color_rules: ColorRules,
        settings: &Settings,
        #[cfg(feature = "logging")] event_tx: Sender<Event>,
    ) -> Self {
        let line_ending: LineEnding = line_ending.into();

        let rendering = settings.rendering.clone();
        #[cfg(feature = "defmt")]
        let defmt = settings.defmt.clone();
        #[cfg(feature = "logging")]
        let logging = settings.logging.clone();

        #[cfg(feature = "logging")]
        let (log_handle, log_thread) = LoggingHandle::new(
            line_ending.clone(),
            logging.clone(),
            #[cfg(feature = "logging")]
            event_tx,
            #[cfg(feature = "defmt")]
            defmt.clone(),
        );

        Self {
            raw: RawBuffer {
                inner: Vec::with_capacity(1024),
                buffer_timestamps: Vec::with_capacity(1024),
                consumed_up_to: 0,
            },
            styled_lines: StyledLines {
                rx: Vec::with_capacity(1024),
                tx: Vec::with_capacity(1024),
            },
            cached_combined_height: Cell::new(None),

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
        }
    }

    /// The public interface where newly recieved bytes are sent.
    pub fn fresh_rx_bytes(&mut self, timestamp: DateTime<Local>, bytes: Vec<u8>) {
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);

        // First append the new bytes to the raw buffer
        self.raw.feed(&bytes, timestamp);

        #[cfg(feature = "logging")]
        // And send them to the logging thread if needed
        self.log_handle.log_rx_bytes(timestamp, bytes).unwrap();

        // let meow = std::time::Instant::now();

        // And *then* do the work to consume them as text/defmt.
        self.consume_latest_bytes(timestamp);
        self.invalidate_height_cache();
        // error!("{:?}", meow.elapsed());

        // self.raw.inner.extend(bytes.iter());
        // while self.consume_latest_bytes(Some(now)) {
        //     ();
        // }
        #[cfg(debug_assertions)]
        {
            let buffer_bytes = self.raw.inner.len();
            debug!("Buffer size: {:.2} KB", buffer_bytes as f64 / 1024.0);
        }
    }

    /// Check if the latest bytes are ready to be consumed
    /// (which might not always be the case, such as when a defmt frame is sent in chunks, it wont be ready until terminated).
    fn consume_latest_bytes(&mut self, timestamp: DateTime<Local>) {
        #[cfg(not(feature = "defmt"))]
        while let Some((_index_in_buffer, delimited_slice)) = self.raw.next_slice_raw() {
            #[allow(irrefutable_let_patterns)] // Other variants are conditionally compiled.
            let DelimitedSlice::Unknown(slice) = delimited_slice else {
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

            self.styled_lines
                .consume_as_text(&self.raw, &self.color_rules, kit, &self.line_ending);
            self.raw.consumed(slice.len());
        }

        #[cfg(feature = "defmt")]
        while let Some((_index_in_buffer, delimited_slice)) = self
            .raw
            .next_slice_checked(self.defmt_settings.defmt_parsing.clone())
            && !self.defmt_raw_malformed
        {
            match delimited_slice {
                DelimitedSlice::Unknown(slice) => {
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
                        kit,
                        &self.line_ending,
                    );
                    self.raw.consumed(slice.len());
                }
                DelimitedSlice::DefmtRaw(raw_uncompressed) => {
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
                    if let Some(decoder) = &self.defmt_decoder {
                        match decoder.table.decode(raw_uncompressed) {
                            Ok((frame, consumed)) => {
                                self.styled_lines.consume_frame(
                                    kit,
                                    decoder,
                                    &frame,
                                    &self.color_rules,
                                );

                                self.raw.consumed(consumed);
                            }
                            Err(defmt_decoder::DecodeError::UnexpectedEof) => {
                                break;
                            }
                            Err(defmt_decoder::DecodeError::Malformed) => {
                                self.defmt_raw_malformed = true;
                                self.styled_lines.failed_decode(
                                    delimited_slice,
                                    DefmtPacketError::MalformedRawFrame,
                                    kit,
                                    &self.line_ending,
                                );
                                break;
                            }
                        }
                    } else {
                        let slice_len = delimited_slice.raw_len();
                        self.styled_lines.failed_decode(
                            delimited_slice,
                            DefmtPacketError::NoDecoder,
                            kit,
                            &self.line_ending,
                        );
                        self.raw.consumed(slice_len);
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

                    self.raw.consumed(raw_slice_len);
                }
            }
        }
    }

    /// Clears recieved styled lines and reconsumes the whole
    /// `raw_buffer` again, using the same rx chunk sizes as they were
    /// originally recieved in order to preserve behavior of incomplete lines getting
    /// interrupted by a user input midway.
    ///
    /// Avoid running when possible, isn't cheap to run.
    pub fn reconsume_raw_buffer(&mut self) {
        if self.raw.inner.is_empty() {
            warn!("Can't reconsume an empty buffer!");
            return;
        }

        self.styled_lines.rx.clear();

        let user_timestamps: Vec<_> = self
            .styled_lines
            .tx
            .iter()
            .map(|b| {
                let LineType::User {
                    is_bytes,
                    #[cfg(feature = "macros")]
                    is_macro,
                    ..
                } = &b.line_type
                else {
                    unreachable!("only user lines should be in here");
                };

                (
                    LineType::User {
                        is_bytes: *is_bytes,
                        #[cfg(feature = "macros")]
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
        // Replacing with a blank copy so we can call the same `&mut self` methods.
        let rx_buffer = std::mem::replace(
            &mut self.raw,
            RawBuffer::with_capacities(orig_buf_len, timestamps_len),
        );
        let user_echo = self.rendering.echo_user_input;

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
                .map(|(index, timestamp, _len)| (*index, *timestamp, false))
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

        for x in buffer_slices {
            let (slice, timestamp, was_user_line, _range) = x;
            // If this was where a user line we allow to render is,
            // then we'll finish this line early if it's not already finished.
            if was_user_line {
                if let Some(last_rx) = self.styled_lines.rx.last_mut()
                    && matches!(
                        last_rx.line_type,
                        LineType::Port(LineFinished::Unfinished { .. })
                    )
                {
                    last_rx.line_type = LineType::Port(LineFinished::CutShort);
                }
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

        self.scroll_by(0);
    }

    #[cfg(feature = "logging")]
    /// Relog contents in buffer
    pub fn relog_buffer(&mut self) -> Result<(), LoggingWorkerMissing> {
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
            .filter(|_| self.log_settings.log_user_input)
            .map(|b| {
                let LineType::User { .. } = &b.line_type else {
                    unreachable!();
                };

                (b.line_type.clone(), b.range().start, b.timestamp)
            })
            .collect();
        let orig_buf_len = self.raw.inner.len();

        let blank_port_line_type = LineType::Port(LineFinished::CutShort);
        // Getting all time-tagged indices in the buffer where either
        // 1. Data came in through the port
        // 2. The user sent data
        let interleaved_points = interleave_by(
            self.raw
                .buffer_timestamps
                .iter()
                .map(|(index, timestamp, _len)| (*index, *timestamp, blank_port_line_type.clone()))
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
        use crate::buffer::{
            buf_line::LineFinished,
            logging::{SyncBatch, TxPayload},
        };

        self.log_handle.begin_relogging(relog_rx)?;

        let mut rx_batch = Vec::new();
        let mut tx_batch = Vec::new();

        for (slice, timestamp, line_type, (_slice_start, _slice_end)) in buffer_slices {
            // If this was where a user line we allow to render is,
            // then we'll finish this line early if it's not already finished.
            if let LineType::User {
                is_bytes: _,
                #[cfg(feature = "macros")]
                    is_macro: _,
                escaped_line_ending,
                reloggable_raw,
            } = line_type
            {
                if !rx_batch.is_empty() {
                    let transmitted_rx = std::mem::take(&mut rx_batch);
                    relog_tx.send(SyncBatch::RxBatch(transmitted_rx))?;
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
                    relog_tx.send(SyncBatch::TxBatch(transmitted_tx))?;
                }

                rx_batch.push((timestamp, slice.to_owned()));
            }
        }

        if !rx_batch.is_empty() {
            relog_tx.send(SyncBatch::RxBatch(rx_batch))?;
        } else if !tx_batch.is_empty() {
            relog_tx.send(SyncBatch::TxBatch(tx_batch))?;
        }

        relog_tx.send(SyncBatch::Done)?;

        Ok(())
    }

    // pub fn update_line_ending(&mut self, line_ending: &str) {
    pub fn update_line_ending(&mut self, line_ending: &[u8]) {
        self.invalidate_height_cache();
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
        self.invalidate_height_cache();
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
        self.invalidate_height_cache();
        let old = std::mem::replace(&mut self.defmt_settings, defmt);
        let new = &self.defmt_settings;
        let should_reconsume = changed!(old, new, defmt_parsing);

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
    pub fn update_logging_settings(
        &mut self,
        logging: Logging,
    ) -> Result<(), LoggingWorkerMissing> {
        self.log_handle.update_settings(logging)?;
        Ok(())
    }

    /// User is returning to port selection, clean everything up.
    pub fn intentional_disconnect_clear(&mut self) -> color_eyre::Result<()> {
        #[cfg(feature = "logging")]
        self.log_handle.log_port_disconnected(true)?;

        self.styled_lines.rx.clear();
        self.styled_lines.rx.shrink_to(1024);

        self.styled_lines.tx.clear();
        self.styled_lines.tx.shrink_to(1024);

        self.raw.reset();

        Ok(())
    }

    /// User sent an input in Pseudo-shells byte mode, or a macro with escaped bytes.
    pub fn append_user_bytes(
        &mut self,
        bytes: &[u8],
        line_ending_bytes: &[u8],
        #[cfg(feature = "macros")] macro_sensitivity: Option<bool>,
    ) {
        let now = Local::now();
        let user_span = span!(Color::DarkGray; "BYTE> ");

        #[cfg(not(feature = "macros"))]
        let macro_sensitivity = None;

        // If input is a macro and is marked sensitive, just replace every shown byte with a *
        let text = if let Some(true) = macro_sensitivity {
            span!(Style::new().dark_gray(); "*".repeat(bytes.len()))
        } else {
            let text: Span = bytes
                .iter()
                .chain(line_ending_bytes.iter())
                .map(|b| format!("\\x{b:02X}"))
                .join("")
                .into();

            text.dark_gray().italic().bold()
        };

        let line = Line::from(vec![user_span, text]);

        let tx_line_ending: LineEnding = line_ending_bytes.into();

        let kit = BufLineKit {
            timestamp: now,
            area_width: self.last_terminal_size.width,
            render: self.line_render_settings(),
            full_range_slice: RangeSlice {
                range: self.raw.inner.len()..self.raw.inner.len(),
                slice: &[],
            },
        };
        let reloggable_raw = bytes
            .iter()
            .chain(line_ending_bytes.iter())
            .copied()
            .collect::<Vec<u8>>();
        let user_buf_line = BufLine::user_line(
            line,
            kit,
            &tx_line_ending,
            true,
            #[cfg(feature = "macros")]
            macro_sensitivity.is_some(),
            reloggable_raw,
        );

        // If we're going to show this input, cut any unterminated port line short.
        if let Some(last_rx) = self.styled_lines.rx.last_mut()
            && matches!(
                last_rx.line_type,
                LineType::Port(LineFinished::Unfinished { .. })
            )
            && self
                .rendering
                .echo_user_input
                .filter_user_line(&user_buf_line.line_type)
        {
            last_rx.line_type = LineType::Port(LineFinished::CutShort);
        }

        #[cfg(feature = "logging")]
        if self.log_settings.log_text_to_file && self.log_settings.log_user_input {
            self.log_handle
                .log_tx_bytes(
                    now,
                    bytes.to_owned(),
                    line_ending_bytes.as_bytes().to_owned(),
                )
                .expect("Logging worker has disappeared!");
        }
        self.styled_lines.tx.push(user_buf_line);
        self.invalidate_height_cache();
    }

    pub fn append_user_text(
        &mut self,
        text: &str,
        line_ending_bytes: &[u8],
        #[cfg(feature = "macros")] macro_sensitivity: Option<bool>,
    ) {
        let now = Local::now();

        let tx_line_ending: LineEnding = line_ending_bytes.into();

        let user_span = span!(Color::DarkGray;"USER> ");

        // TODO HANDLE MULTI-LINE USER INPUT IN THE UI AAAAAAAAA
        for (trunc, _orig, _range) in line_ending_iter(text.as_bytes(), &tx_line_ending) {
            #[cfg(not(feature = "macros"))]
            let macro_sensitivity = None;

            let line = if let Some(true) = macro_sensitivity {
                let text = span!(Style::new().dark_gray(); "*".repeat(trunc.len()));
                Line::from(vec![user_span.clone(), text])
            } else {
                let mut line = Line::from(String::from_utf8_lossy(trunc).to_string());
                line.spans.insert(0, user_span.clone());
                line.style_all_spans(Color::DarkGray.into());
                line
            };

            let kit = BufLineKit {
                timestamp: now,
                area_width: self.last_terminal_size.width,
                render: self.line_render_settings(),
                full_range_slice: RangeSlice {
                    range: self.raw.inner.len()..self.raw.inner.len(),
                    slice: &[],
                },
            };
            let reloggable_raw = trunc
                .iter()
                .chain(line_ending_bytes.iter())
                .copied()
                .collect::<Vec<u8>>();
            let user_buf_line = BufLine::user_line(
                line,
                kit,
                &tx_line_ending,
                false,
                #[cfg(feature = "macros")]
                macro_sensitivity.is_some(),
                reloggable_raw,
            );

            // If we're going to show this input, cut any unterminated port line short.
            if let Some(last_rx) = self.styled_lines.rx.last_mut()
                && matches!(
                    last_rx.line_type,
                    LineType::Port(LineFinished::Unfinished { .. })
                )
                && self
                    .rendering
                    .echo_user_input
                    .filter_user_line(&user_buf_line.line_type)
            {
                last_rx.line_type = LineType::Port(LineFinished::CutShort);
            }

            #[cfg(feature = "logging")]
            if self.log_settings.log_text_to_file && self.log_settings.log_user_input {
                self.log_handle
                    .log_tx_bytes(
                        now,
                        text.as_bytes().to_owned(),
                        tx_line_ending.as_bytes().to_owned(),
                    )
                    .expect("Logging worker has disappeared!");
            }
            self.styled_lines.tx.push(user_buf_line);
        }
        self.invalidate_height_cache();
    }
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
) -> impl Iterator<Item = (&'a [u8], &'a [u8], Range<usize>)> {
    match line_ending {
        LineEnding::None => Either::Left(std::iter::once((bytes, bytes, 0..bytes.len()))),
        line_ending => Either::Right(_line_ending_iter(bytes, line_ending)),
    }
}

// Internal impl, for actually iterating through line endings.
#[inline(always)]
fn _line_ending_iter<'a>(
    bytes: &'a [u8],
    line_ending: &'a LineEnding,
) -> impl Iterator<Item = (&'a [u8], &'a [u8], Range<usize>)> {
    let line_ending_pos_iter = match line_ending {
        LineEnding::None => unreachable!("line_ending cannot be empty"),
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
                last_index..bytes.len(),
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
                index_copy..line_ending_index + le_len,
            )
        };
        Some(result)
    })
}
