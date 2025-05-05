use std::{borrow::Cow, cmp::Ordering, collections::BTreeSet, iter::Peekable};

use ansi_to_tui::IntoText;
use buf_line::BufLine;
use chrono::{DateTime, Local};
use itertools::{Either, Itertools};
use memchr::memmem::Finder;
use ratatui::{
    layout::Size,
    style::{Color, Style, Stylize, palette::material::PINK},
    text::{Line, Span, Text, ToText},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget, Wrap,
    },
};
use ratatui_macros::{line, span};
use tracing::{debug, error, info, warn};

use crate::{
    errors::YapResult,
    traits::{ByteSuffixCheck, LineHelpers},
};

mod buf_line;
mod wrap;

// use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct BufferState {
    text_wrapping: bool,
    // TODO maybe make this private and provide a function that auto-runs the render length and scroll..?
    timestamps_visible: bool,
    user_echo_input: UserEcho,
    vert_scroll: usize,
    scrollbar_state: ScrollbarState,
    stuck_to_bottom: bool,
}

impl UserEcho {
    fn filter_user_line(&self, buf_line: &BufLine) -> bool {
        match self {
            UserEcho::None => false,
            UserEcho::All => true,
            UserEcho::NoBytes => !buf_line.is_bytes,
            UserEcho::NoMacros => !buf_line.is_macro,
            UserEcho::NoMacrosOrBytes => !buf_line.is_bytes && !buf_line.is_macro,
        }
    }
}

// TODO have separate vector for user lines, and re-render the raw buffer when turning user lines on and off?

pub struct Buffer {
    raw_buffer: Vec<u8>,
    // Time-tagged indexes into `raw_buffer`, from each input from the port.
    buffer_timestamps: Vec<(usize, DateTime<Local>)>,
    lines: Vec<BufLine>,
    user_lines: Vec<BufLine>,
    last_line_completed: bool,

    /// The last-known size of the area given to render the buffer in
    last_terminal_size: Size,

    // pub color_rules
    pub state: BufferState,

    // TODO separate line ending for TX'd text?
    pub line_ending: String,
    // line_ending_finder: Finder<'static>,
    #[cfg(debug_assertions)]
    pub debug_lines: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, strum::Display,
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
        line_ending: &str,
        text_wrapping: bool,
        timestamps_visible: bool,
        user_echo: UserEcho,
    ) -> Self {
        Self {
            raw_buffer: Vec::with_capacity(1024),
            buffer_timestamps: Vec::with_capacity(1024),
            lines: Vec::with_capacity(1024),
            user_lines: Vec::with_capacity(1024),
            last_terminal_size: Size::default(),
            state: BufferState {
                vert_scroll: 0,
                scrollbar_state: ScrollbarState::default(),
                stuck_to_bottom: false,
                text_wrapping,
                timestamps_visible,
                user_echo_input: user_echo,
            },
            line_ending: line_ending.to_owned(),
            last_line_completed: true,
            #[cfg(debug_assertions)]
            debug_lines: false,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    pub fn append_user_bytes(&mut self, bytes: &[u8], is_macro: bool) {
        let now = Local::now();
        let text: Span = bytes.iter().map(|b| format!("{:02X}", b)).join(" ").into();
        let text = text.dark_gray().italic().bold();

        let user_span = span!(Color::DarkGray; "BYTE> ");

        let line = Line::from(vec![user_span, text]);

        // line.spans.insert(0, user_span.clone());
        // line.style_all_spans(Color::DarkGray.into());
        let user_buf_line = BufLine::new_with_line(
            line,
            #[cfg(debug_assertions)]
            bytes,
            self.raw_buffer.len(), // .max(1)
            self.last_terminal_size.width,
            self.state.timestamps_visible,
            #[cfg(debug_assertions)]
            self.debug_lines,
            now,
            true,
            is_macro,
        );
        self.last_line_completed = self.state.user_echo_input.filter_user_line(&user_buf_line)
            || self.raw_buffer.has_byte_suffix(self.line_ending.as_bytes());
        self.user_lines.push(user_buf_line);
        // TODO make this more dynamic with the macro hiding
    }

    pub fn append_user_text(&mut self, text: &str, is_macro: bool) {
        let now = Local::now();
        let mm = text.escape_debug().to_string();

        let user_span = span!(Color::DarkGray;"USER> ");
        // let Text { lines, .. } = text;
        // TODO HANDLE MULTI-LINE USER INPUT AAAA
        for (trunc, orig, _indices) in line_ending_iter(mm.as_bytes(), &self.line_ending) {
            let mut line = match trunc.into_line_lossy(None, Style::new()) {
                Ok(line) => line,
                Err(_) => {
                    error!("ansi-to-tui failed to parse input! Using unstyled text.");
                    Line::from(String::from_utf8_lossy(trunc).to_string())
                }
            };

            line.spans.insert(0, user_span.clone());
            line.style_all_spans(Color::DarkGray.into());
            let user_buf_line = BufLine::new_with_line(
                line,
                #[cfg(debug_assertions)]
                orig,
                self.raw_buffer.len(), // .max(1)
                self.last_terminal_size.width,
                self.state.timestamps_visible,
                #[cfg(debug_assertions)]
                self.debug_lines,
                now,
                false,
                is_macro,
            );
            // Used to be out of the for loop.
            self.last_line_completed = self.state.user_echo_input.filter_user_line(&user_buf_line)
                || self.raw_buffer.has_byte_suffix(self.line_ending.as_bytes());

            self.user_lines.push(user_buf_line);
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
        let append_to_last = !self.last_line_completed;
        if orig.is_empty() {
            return;
        }

        if append_to_last {
            let last_line = self.lines.last_mut().expect("can't append to nothing");
            let last_index = last_line.index_in_buffer();

            let slice = &self.raw_buffer[last_index..start_index + trunc.len()];
            // info!("AAAFG: {:?}", slice);
            let line = match slice.into_line_lossy(None, Style::new()) {
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
            last_line.update_line(
                line,
                #[cfg(debug_assertions)]
                slice,
                self.last_terminal_size.width,
                self.state.timestamps_visible,
                #[cfg(debug_assertions)]
                self.debug_lines,
            );
        } else {
            let line = match trunc.into_line_lossy(None, Style::new()) {
                Ok(line) => line,
                Err(_) => {
                    error!("ansi-to-tui failed to parse input! Using unstyled text.");
                    Line::from(String::from_utf8_lossy(trunc).to_string())
                }
            };

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
            self.lines.push(BufLine::new_with_line(
                line,
                #[cfg(debug_assertions)]
                orig,
                start_index,
                self.last_terminal_size.width,
                self.state.timestamps_visible,
                #[cfg(debug_assertions)]
                self.debug_lines,
                known_time.unwrap_or_else(Local::now),
                false,
                false,
            ));
        };
        self.last_line_completed = {
            // let last_line = self.lines.last().expect("expected at least one line");
            let expected_ending = self.line_ending.as_bytes();
            self.raw_buffer.has_byte_suffix(expected_ending)
        };
    }

    // Forced to use Vec<u8> for now
    pub fn append_rx_bytes(&mut self, bytes: &mut Vec<u8>) {
        let now = Local::now();
        let mut index = self.raw_buffer.len();
        self.buffer_timestamps.push((index, now));
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);

        for (trunc, orig, indices) in line_ending_iter(bytes, self.line_ending.clone().as_str()) {
            index = self.raw_buffer.len();
            self.raw_buffer.extend(orig);
            self.consume_port_bytes(trunc, orig, index, Some(now));
        }
    }

    /// Clears `self.lines` and reiterates through the whole `raw_buffer` again.
    ///
    /// Avoid running when possible, isn't cheap to run.
    pub fn reconsume_raw_buffer(&mut self) {
        if self.raw_buffer.is_empty() {
            warn!("Can't reconsume an empty buffer!");
            return;
        }

        // let _ = std::mem::take(&mut self.lines);
        self.lines.clear();

        // Taking these variables out of `self` temporarily to allow running &mut self methods while holding
        // references to these.
        let timestamps = std::mem::take(&mut self.buffer_timestamps);
        let user_lines = std::mem::take(&mut self.user_lines);
        let orig_buf_len = self.raw_buffer.len();
        let buffer = std::mem::replace(&mut self.raw_buffer, Vec::with_capacity(orig_buf_len));
        let line_ending = self.line_ending.clone();
        let user_echo = self.state.user_echo_input.clone();

        // No lines to append to.
        self.last_line_completed = true;

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
            |device, user| match device.0.cmp(&user.0) {
                Ordering::Equal => device.1 <= user.1,
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
                self.last_line_completed = true;
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
                self.raw_buffer.extend(orig);
                self.consume_port_bytes(trunc, orig, new_index, Some(timestamp));
                new_index += orig.len();
            }
        }

        assert_eq!(
            orig_buf_len,
            self.raw_buffer.len(),
            "Buffer size should not have changed during reconsumption."
        );

        assert_eq!(
            new_index, orig_buf_len,
            "Iterator's slices should have same total length as raw buffer."
        );

        self.buffer_timestamps = timestamps;
        self.user_lines = user_lines;
        self.scroll_by(0);
    }

    /// Updates each BufLine's render height with the new terminal width, returning the sum total at the end
    pub fn update_wrapped_line_heights(&mut self) -> usize {
        self.lines.iter_mut().fold(0, |total, l| {
            let new_height = l.update_line_height(
                self.last_terminal_size.width,
                self.state.timestamps_visible,
                #[cfg(debug_assertions)]
                self.debug_lines,
            );

            total + new_height
        }) + self.user_lines.iter_mut().fold(0, |total, l| {
            let new_height = l.update_line_height(
                self.last_terminal_size.width,
                self.state.timestamps_visible,
                #[cfg(debug_assertions)]
                self.debug_lines,
            );

            total + new_height
        })
    }
    pub fn set_line_wrap(&mut self, wrap: bool) {
        self.state.text_wrapping = wrap;
        self.scroll_by(0);
    }
    pub fn set_user_lines(&mut self, user_echo: UserEcho) {
        self.state.user_echo_input = user_echo;
        self.reconsume_raw_buffer();
    }
    fn buflines_iter(&self) -> impl Iterator<Item = &BufLine> {
        if self.state.user_echo_input == UserEcho::None {
            Either::Left(self.lines.iter())
        } else {
            Either::Right(interleave(
                self.lines.iter(),
                self.user_lines
                    .iter()
                    .filter(|l| self.state.user_echo_input.filter_user_line(l)),
            ))
        }
    }
    pub fn lines_iter(&self) -> (impl Iterator<Item = Line>, u16) {
        // TODO styling based on line prefix
        // or have BufLine.value be an enum for String/ratatui::Line
        // and then match against at in BufLine::as_line()
        let last_size = &self.last_terminal_size;
        let total_lines = self.combined_height();
        let more_lines_than_height = total_lines > last_size.height as usize;

        // let lines_iter = ;

        let entries_to_skip: usize;
        let entries_to_take: usize;

        let mut wrapped_scroll: u16 = 0;

        if more_lines_than_height {
            let desired_visible_lines = last_size.height as usize;
            if self.state.text_wrapping {
                let vert_scroll = self.state.vert_scroll;
                let (spillover_index, spillover_lines_visible, spilt_line_total_height) = {
                    let mut current_line_index: usize = 0;
                    let mut current_line_height: usize = 0;

                    let mut lines_from_top: usize = 0;
                    for (index, entries_lines) in self
                        .buflines_iter()
                        .map(|l| l.get_line_height())
                        .enumerate()
                    {
                        current_line_index = index;
                        current_line_height = entries_lines;

                        lines_from_top += entries_lines;
                        if lines_from_top > vert_scroll {
                            break;
                        }
                    }

                    let visible_lines = lines_from_top - vert_scroll;

                    let spillover_lines = if current_line_height == visible_lines {
                        // If we can see all of the lines of this entry, then it's not spilling over
                        0
                    } else {
                        wrapped_scroll = (current_line_height - visible_lines) as u16;
                        // Returns how many lines are visibly spilling over from the
                        // entry being cropped by the top of the buffer window.
                        visible_lines
                    };

                    (current_line_index, spillover_lines, current_line_height)
                };

                // debug!("scroll: {vert_scroll}, index: {spillover_index}, spillover lines: {spillover_lines_visible}, wrapped scroll: {wrapped_scroll}");

                entries_to_skip = spillover_index;
                entries_to_take = {
                    let mut visible_lines: isize = -(spilt_line_total_height as isize);
                    let mut entries_to_take = 0;

                    for entry_lines in self
                        .buflines_iter()
                        .skip(entries_to_skip)
                        .map(|l| l.get_line_height())
                    {
                        entries_to_take += 1;
                        visible_lines += entry_lines as isize;

                        if visible_lines > desired_visible_lines as isize {
                            // debug!(
                            //     "visible_lines: {visible_lines}, desired: {desired_visible_lines}"
                            // );
                            break;
                        }
                    }

                    // debug!(
                    //     "entries_to_skip: {entries_to_skip}, entries_to_take: {entries_to_take}"
                    // );

                    entries_to_take
                };
            } else {
                entries_to_skip = self.state.vert_scroll;
                // self.lines.len() - last_size.height as usize;
                entries_to_take = desired_visible_lines;
            }
        } else {
            entries_to_skip = 0;
            entries_to_take = usize::MAX;
        }

        (
            self.buflines_iter()
                .skip(entries_to_skip)
                .take(entries_to_take)
                .map(|l| {
                    l.as_line(
                        self.state.timestamps_visible,
                        #[cfg(debug_assertions)]
                        self.debug_lines,
                    )
                }),
            wrapped_scroll,
        )
    }

    pub fn terminal_paragraph(&self) -> Paragraph<'_> {
        let (lines_iter, vert_scroll) = self.lines_iter();
        let lines: Vec<_> = lines_iter.collect();
        let para = Paragraph::new(lines)
            .block(Block::new().borders(Borders::RIGHT))
            .scroll((vert_scroll, 0));
        if self.state.text_wrapping {
            para.wrap(Wrap { trim: false })
        } else {
            para
        }
    }
    pub fn clear(&mut self) {
        self.lines.clear();
        self.buffer_timestamps.clear();
        self.user_lines.clear();
        self.raw_buffer.clear();
        self.last_line_completed = true;
    }

    pub fn scroll_page_up(&mut self) {
        let amount = self.last_terminal_size.height - 2;
        self.scroll_by(amount as i32);
    }

    pub fn scroll_page_down(&mut self) {
        let amount = self.last_terminal_size.height - 2;
        let amount = -(amount as i32);
        self.scroll_by(amount);
    }

    pub fn scroll_by(&mut self, up: i32) {
        match up {
            0 => (), // Used to trigger scroll update actions from non-user scrolling events.
            // Scroll all the way up
            i32::MAX => {
                self.state.vert_scroll = 0;
                self.state.stuck_to_bottom = false;
            }
            // Scroll all the way down
            i32::MIN => self.state.vert_scroll = self.combined_height(),

            // Scroll up
            x if up > 0 => {
                self.state.vert_scroll = self.state.vert_scroll.saturating_sub(x as usize);
            }
            // Scroll down
            x if up < 0 => {
                self.state.vert_scroll = self.state.vert_scroll.saturating_add(x.abs() as usize);
            }
            _ => unreachable!(),
        }

        let last_size = &self.last_terminal_size;
        let total_lines = self.combined_height();
        let more_lines_than_height = total_lines > last_size.height as usize;

        if up > 0 && more_lines_than_height {
            self.state.stuck_to_bottom = false;
        } else if self.state.vert_scroll + last_size.height as usize >= self.combined_height() {
            self.state.vert_scroll = self.combined_height();
            self.state.stuck_to_bottom = true;
        }

        if self.state.stuck_to_bottom {
            let new_pos = total_lines.saturating_sub(last_size.height as usize);
            self.state.vert_scroll = new_pos;
        }
        self.state.scrollbar_state = self
            .state
            .scrollbar_state
            .position(self.state.vert_scroll)
            .content_length(
                self.combined_height()
                    .saturating_sub(last_size.height as usize),
            );
    }
    // fn wrapped_line_count(&self) -> usize {
    //     self.buflines_iter().map(|l| l.get_line_height()).sum()
    // }

    /// Returns the total amount of lines that can be rendered,
    /// taking into account if text wrapping is enabled or not.
    pub fn combined_height(&self) -> usize {
        if self.state.text_wrapping {
            self.buflines_iter().map(|l| l.get_line_height()).sum()
        } else {
            self.buflines_iter().count()
        }
    }

    pub fn port_lines_len(&self) -> usize {
        self.lines.len()
    }

    pub fn update_terminal_size(
        &mut self,
        terminal: &mut ratatui::Terminal<impl ratatui::prelude::Backend>,
    ) -> YapResult<()> {
        self.last_terminal_size = {
            let mut terminal_size = terminal.size().unwrap();
            // `2` is the lines from the repeating_pattern_widget and the input buffer.
            // Might need to make more dynamic later?
            terminal_size.height = terminal_size.height.saturating_sub(2);
            terminal_size
        };
        self.update_wrapped_line_heights();
        self.scroll_by(0);
        Ok(())
    }

    // pub fn line_ending(&self) -> &str {
    //     &self.line_ending
    // }

    pub fn show_timestamps(&mut self, visible: bool) -> usize {
        self.state.timestamps_visible = visible;
        let count = self.update_wrapped_line_heights();
        self.scroll_by(0);
        count
    }
}

// TODO make tests for this idiot thing

/// Returns an iterator over the given byte slice, seperated by (and excluding) the given line ending `&str`.
///
/// String slice tuple is in order of `(exclusive, inclusive/original)`.
///
/// `usize` tuple has the inclusive indices into the given slice.
///
/// If no line ending was found, emits the whole slice once.
pub fn line_ending_iter<'a>(
    bytes: &'a [u8],
    line_ending: &'a str,
) -> impl Iterator<Item = (&'a [u8], &'a [u8], (usize, usize))> {
    assert!(!line_ending.is_empty(), "line_ending can't be empty");

    let line_ending = line_ending.as_bytes();

    let line_ending_pos_iter = if line_ending.len() == 1 {
        Either::Left(memchr::memchr_iter(line_ending[0], bytes))
    } else {
        Either::Right(memchr::memmem::find_iter(bytes, line_ending))
    };

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
                last_index = line_ending_index + line_ending.len();
                (
                    &bytes[index_copy..line_ending_index],
                    &bytes[index_copy..line_ending_index + line_ending.len()],
                    (index_copy, line_ending_index + line_ending.len()),
                )
            };
            Some(result)
        });

    slices_iter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_line() {
        let s = b"hello";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, b"hello");
        assert_eq!(res[0].1, b"hello");
        assert_eq!(res[0].2, (0, 5));
    }

    #[test]
    fn test_simple_lines() {
        let s = b"foo\nbar\nbaz";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].0, b"foo");
        assert_eq!(res[0].1, b"foo\n");
        assert_eq!(res[0].2, (0, 4));
        assert_eq!(res[1].0, b"bar");
        assert_eq!(res[1].1, b"bar\n");
        assert_eq!(res[1].2, (4, 8));
        assert_eq!(res[2].0, b"baz");
        assert_eq!(res[2].1, b"baz");
        assert_eq!(res[2].2, (8, 11));
    }

    #[test]
    fn test_few_bytes() {
        let s = b"a";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, b"a");
        assert_eq!(res[0].1, b"a");

        let s = b"";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, b"");
        assert_eq!(res[0].1, b"");
    }

    #[test]
    fn test_trailing_newline() {
        let s = b"a\nb\nc\n";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].0, b"a");
        assert_eq!(res[0].1, b"a\n");
        assert_eq!(res[1].0, b"b");
        assert_eq!(res[1].1, b"b\n");
        assert_eq!(res[2].0, b"c");
        assert_eq!(res[2].1, b"c\n");
    }

    #[test]
    fn test_starting_newline() {
        let s = b"\rb\nc\n";
        let it = line_ending_iter(s, "\r");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, b"");
        assert_eq!(res[0].1, b"\r");
        assert_eq!(res[1].0, b"b\nc\n");
        assert_eq!(res[1].1, b"b\nc\n");
    }

    #[test]
    fn test_crlf() {
        let s = b"one\r\ntwo\r\nthree";
        let it = line_ending_iter(s, "\r\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].0, b"one");
        assert_eq!(res[0].1, b"one\r\n");
        assert_eq!(res[1].0, b"two");
        assert_eq!(res[1].1, b"two\r\n");
        assert_eq!(res[2].0, b"three");
        assert_eq!(res[2].1, b"three");
    }

    #[test]
    #[should_panic(expected = "line_ending can't be empty")]
    fn test_line_ending_empty() {
        let s = b"test";
        let _ = line_ending_iter(s, "");
    }

    #[test]
    fn test_multi_byte_line_ending() {
        let s = b"abcXYZdefXYZghi";
        let it = line_ending_iter(s, "XYZ");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].0, b"abc");
        assert_eq!(res[0].1, b"abcXYZ");
        assert_eq!(res[1].0, b"def");
        assert_eq!(res[1].1, b"defXYZ");
        assert_eq!(res[2].0, b"ghi");
        assert_eq!(res[2].1, b"ghi");
    }

    #[test]
    fn test_multiple_consecutive_line_endings() {
        let s = b"foo\n\nbar\n";
        let it = line_ending_iter(s, "\n");
        let res: Vec<_> = it.collect();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0].0, b"foo");
        assert_eq!(res[0].1, b"foo\n");
        assert_eq!(res[1].0, b"");
        assert_eq!(res[1].1, b"\n");
        assert_eq!(res[2].0, b"bar");
        assert_eq!(res[2].1, b"bar\n");
    }
}

// pub fn colored_line<'a, L: Into<Line<'a>>>(text: L) -> Line<'a> {
//     if text.
//     let line = Line::from(text);

// }

/// Maybe StatefulWidget would make more sense? Unsure.
impl Widget for &mut Buffer {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        // TODO allow this to work
        // self.last_terminal_size = area.as_size();

        let para = self.terminal_paragraph();
        para.render(area, buf);

        if !self.state.stuck_to_bottom {
            let scroll_notice = Line::raw("More... Shift+PgDn to jump to newest").dark_gray();
            let notice_area = {
                let mut rect = area.clone();
                rect.y = rect.bottom().saturating_sub(1);
                rect.height = 1;
                rect
            };
            Clear.render(notice_area, buf);
            scroll_notice.render(notice_area, buf);
        }

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        scrollbar.render(area, buf, &mut self.state.scrollbar_state);
    }
}

fn extract_line(text: Text<'_>) -> Option<Line<'_>> {
    if text.lines.is_empty() {
        return None;
    }
    let Text { lines, .. } = text;
    lines.into_iter().find(|l| !l.spans.is_empty())
}

fn interleave<A, B, I>(left: A, right: B) -> impl Iterator<Item = I>
where
    A: Iterator<Item = I>,
    B: Iterator<Item = I>,
    I: Ord,
{
    let mut left = left.peekable();
    let mut right = right.peekable();
    std::iter::from_fn(move || match (left.peek(), right.peek()) {
        (Some(li), Some(ri)) => {
            if li <= ri {
                left.next()
            } else {
                right.next()
            }
        }
        (Some(_), None) => left.next(),
        (None, Some(_)) => right.next(),
        (None, None) => None,
    })
}

fn interleave_by<A, B, I, F>(left: A, right: B, mut decider: F) -> impl Iterator<Item = I>
where
    A: Iterator<Item = I>,
    B: Iterator<Item = I>,
    F: FnMut(&I, &I) -> bool,
{
    let mut left = left.peekable();
    let mut right = right.peekable();
    std::iter::from_fn(move || match (left.peek(), right.peek()) {
        (Some(li), Some(ri)) => {
            if decider(li, ri) {
                left.next()
            } else {
                right.next()
            }
        }
        (Some(_), None) => left.next(),
        (None, Some(_)) => right.next(),
        (None, None) => None,
    })
}
