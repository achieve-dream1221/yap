use std::{borrow::Cow, cmp::Ordering, collections::BTreeSet, iter::Peekable};

use ansi_to_tui::IntoText;
use buf_line::BufLine;
use chrono::{DateTime, Local};
use itertools::Itertools;
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
        let mut append_to_last = !self.last_line_completed;
        if orig.is_empty() {
            return;
        }

        if append_to_last {
            append_to_last = false;
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

            last_line.update_line(
                line,
                #[cfg(debug_assertions)]
                orig,
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
        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);

        for (trunc, orig, indices) in line_ending_iter(bytes, self.line_ending.clone().as_str()) {
            let index = self.raw_buffer.len();
            self.raw_buffer.extend(orig);
            self.buffer_timestamps.push((index, now));
            self.consume_port_bytes(trunc, orig, index, Some(now));
        }
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
        self.lines
            .iter()
            .filter(|_| self.state.user_echo_input == UserEcho::None)
            .chain(
                interleave(
                    self.lines.iter(),
                    self.user_lines
                        .iter()
                        .filter(|l| self.state.user_echo_input.filter_user_line(l)),
                )
                .filter(|_| self.state.user_echo_input != UserEcho::None),
            )
    }
    pub fn lines_iter(&self) -> (impl Iterator<Item = Line>, u16) {
        // TODO styling based on line prefix
        // or have BufLine.value be an enum for String/ratatui::Line
        // and then match against at in BufLine::as_line()
        let last_size = &self.last_terminal_size;
        let total_lines = self.line_count();
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
            i32::MIN => self.state.vert_scroll = self.line_count(),

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
        let total_lines = self.line_count();
        let more_lines_than_height = total_lines > last_size.height as usize;

        if up > 0 && more_lines_than_height {
            self.state.stuck_to_bottom = false;
        } else if self.state.vert_scroll + last_size.height as usize >= self.line_count() {
            self.state.vert_scroll = self.line_count();
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
            .content_length(self.line_count().saturating_sub(last_size.height as usize));
    }
    fn wrapped_line_count(&self) -> usize {
        self.buflines_iter().map(|l| l.get_line_height()).sum()
    }

    /// Returns the total amount of lines that can be rendered,
    /// taking into account if text wrapping is enabled or not.
    pub fn line_count(&self) -> usize {
        if self.state.text_wrapping {
            self.wrapped_line_count()
        } else {
            self.buflines_iter().count()
        }
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

    pub fn reconsume_raw_buffer(&mut self) {
        if self.raw_buffer.is_empty() {
            warn!("Can't reconsume an empty buffer!");
            return;
        }
        let old_lines = std::mem::take(&mut self.lines);

        let timestamps = std::mem::take(&mut self.buffer_timestamps);

        // let bullshit: BTreeSet<_> = timestamps
        //     .iter()
        //     .map(|(index, timestamp)| (*index, *timestamp))
        //     .chain(
        //         old_lines
        //             .into_iter()
        //             .map(|b| (b.raw_buffer_index, b.timestamp)),
        //     )
        //     .collect();

        let buffer_len = self.raw_buffer.len();
        let buffer = std::mem::replace(&mut self.raw_buffer, Vec::with_capacity(buffer_len));

        let user_lines = std::mem::take(&mut self.user_lines);
        let line_ending = self.line_ending.clone();
        let user_echo = self.state.user_echo_input;
        self.last_line_completed = true;

        let interleaved_points = interleave_by(
            timestamps
                .iter()
                .map(|(index, timestamp)| (*index, *timestamp, false))
                // Add a "finale" element to capture any remaining buffer, always placed at the end.
                .chain(std::iter::once((buffer_len, Local::now(), false))),
            user_lines
                .iter()
                .filter(|b| user_echo.filter_user_line(b))
                .map(|b| (b.raw_buffer_index, b.timestamp, true)),
            |device, user| match device.0.cmp(&user.0) {
                Ordering::Equal => device.1 <= user.1,
                Ordering::Less => true,
                Ordering::Greater => false,
            },
        );

        let mut new_index = 0;
        debug!("total len: {buffer_len}");
        // for (start_index, timestamp, is_user) in mrrr {
        //     debug!("{index}..{start_index}, {timestamp}, {is_user}");
        //     index = start_index;
        // }

        let buffer_slices = interleaved_points
            .tuple_windows()
            .filter(|((start_index, _, was_user_line), (end_index, _, _))| {
                start_index != end_index || *was_user_line
            })
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

        // let mut recession_indicator = None;

        for (slice, timestamp, was_user_line, indices) in buffer_slices {
            // if indices.0 == indices.1 {
            // recession_indicator = Some(timestamp);
            // continue;
            // };
            debug!("{indices:?}, {timestamp}, {was_user_line}");
            self.raw_buffer.extend(slice);
            // let timestamp = Some(recession_indicator.take().unwrap_or(timestamp));
            for (trunc, orig, indices) in line_ending_iter(slice, &line_ending) {
                // debug!("{trunc:?}");
                self.consume_port_bytes(trunc, orig, new_index, Some(timestamp));
            }
            new_index += slice.len();

            // If this was where a user line we allow to render is,
            // then we'll finish this line early if it's not already finished.
            if was_user_line {
                self.last_line_completed = true;
            }
        }

        assert_eq!(
            buffer_len,
            self.raw_buffer.len(),
            "Buffer size should not have changed during reconsumption."
        );

        // let indices: Vec<usize> = ;

        // for (trunc, orig, indices) in line_ending_iter(&buffer, self.line_ending.clone().as_str()) {
        //     // Find the timestamp for where this slice started in old_lines
        //     // let known_time = timestamps
        //     //     .iter()
        //     //     .filter(|(index, timestamp)| index <= &indices.0)
        //     //     .max_by_key(|(index, timestamp)| index)
        //     //     .map(|(index, timestamp)| *timestamp);

        //     // let (known_time, force_completed) = timestamps
        //     //     .iter()
        //     //     .map(|(index, timestamp)| (index, timestamp, false))
        //     // .filter(|(index, timestamp, force)| index <= &indices.0)
        //     // .max_by_key(|(index, timestamp, force)| index)
        //     //     .map(|(index, timestamp, force)| (*timestamp, false))
        //     //     .unwrap();

        //     if user_time {
        //         self.last_line_completed = true;
        //     }

        //     self.consume_device_bytes(trunc, orig, indices.0, Some(*known_time));
        // }

        // self.raw_buffer = buffer;
        self.buffer_timestamps = timestamps;
        self.user_lines = user_lines;
        self.scroll_by(0);
    }
}

// TODO make tests for this idiot thing

/// Returns an iterator over the given byte slice, seperated by (and excluding) the given line ending `&str`
///
/// String slice tuple is in order of `(exclusive, inclusive/original)`.
///
/// Returns `None` if there were no matching line endings found.
pub fn line_ending_iter<'a>(
    bytes: &'a [u8],
    line_ending: &'a str,
) -> impl Iterator<Item = (&'a [u8], &'a [u8], (usize, usize))> {
    assert!(!line_ending.is_empty(), "line_ending can't be empty");

    let line_ending = line_ending.as_bytes();

    let line_ending_pos_iter = memchr::memmem::find_iter(bytes, line_ending)
        .map(|line_ending_index| (line_ending_index, false))
        .chain(std::iter::once((bytes.len(), true)));

    let mut last_index = 0;

    let slices_iter = line_ending_pos_iter.map(move |(line_ending_index, is_final_entry)| {
        if is_final_entry {
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
        }
    });

    slices_iter
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
