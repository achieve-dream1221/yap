use std::borrow::Cow;

use ansi_to_tui::IntoText;
use buf_line::BufLine;
use chrono::{DateTime, Local};
use memchr::memmem::Finder;
use ratatui::{
    layout::Size,
    style::{palette::material::PINK, Color, Style, Stylize},
    text::{Line, Span, Text, ToText},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget, Wrap,
    },
};
use ratatui_macros::{line, span};
use tracing::{debug, error, info};

use crate::traits::{ByteSuffixCheck, RemoveUnsavory};

mod buf_line;
mod wrap;

// use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct BufferState {
    pub text_wrapping: bool,
    // TODO maybe make this private and provide a function that auto-runs the render length and scroll..?
    pub timestamps_visible: bool,

    vert_scroll: usize,
    scrollbar_state: ScrollbarState,
    stuck_to_bottom: bool,
}

pub struct Buffer {
    raw_buffer: Vec<u8>,
    pub lines: Vec<BufLine>,
    last_line_completed: bool,

    /// The last-known size of the area given to render the buffer in
    last_terminal_size: Size,

    // pub color_rules
    pub state: BufferState,

    // TODO separate line ending for TX'd text?
    pub line_ending: String,
    // line_ending_finder: Finder<'static>,
}

// impl Default for Buffer {
//     fn default() -> Self {
//         Self::new()
//     }
// }

impl Buffer {
    pub fn new(line_ending: &str) -> Self {
        Self {
            raw_buffer: Vec::with_capacity(1024),
            lines: Vec::with_capacity(1024),
            last_terminal_size: Size::default(),
            state: BufferState {
                vert_scroll: 0,
                scrollbar_state: ScrollbarState::default(),
                stuck_to_bottom: false,
                text_wrapping: false,
                timestamps_visible: false,
            },
            line_ending: line_ending.to_owned(),
            last_line_completed: true,
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    // TODO also do append_user_bytes
    pub fn append_user_text(&mut self, text: &str) {
        let mm = text.escape_debug().to_string();
        let lines: Vec<_> = match line_ending_iter(mm.as_bytes(), &self.line_ending) {
            Some(iter) => iter.collect(),
            None => vec![(mm.as_bytes(), mm.as_bytes())],
        };

        let user_span = span!(Color::DarkGray;"USER> ");
        // let Text { lines, .. } = text;
        // TODO HANDLE MULTI-LINE USER INPUT AAAA
        for (trunc, orig) in lines {
            let mut line = match trunc.into_line_lossy(None, Style::new()) {
                Ok(line) => line,
                Err(_) => {
                    error!("ansi-to-tui failed to parse input! Using unstyled text.");
                    Line::from(String::from_utf8_lossy(trunc).to_string())
                }
            };

            line.spans.insert(0, user_span.clone());
            for span in line.spans.iter_mut() {
                span.style = Color::DarkGray.into();
            }
            self.lines.push(BufLine::new_with_line(
                line,
                0,
                self.last_terminal_size.width,
                self.state.timestamps_visible,
            ));
        }
        self.last_line_completed = true;
    }

    // Forced to use Vec<u8> for now
    pub fn append_rx_bytes(&mut self, bytes: &mut Vec<u8>) {
        let mut append_to_last = !self.last_line_completed;

        let lines: Vec<_> = match line_ending_iter(bytes, &self.line_ending) {
            Some(iter) => iter.collect(),
            None => vec![(bytes, bytes)],
        };

        // debug!("{lines:?}");
        // debug!("{:#?}", self.lines);

        for (trunc, orig) in lines {
            if orig.is_empty() {
                debug!("empty orig!");
                continue;
            }

            let index = self.raw_buffer.len();
            self.raw_buffer.extend(orig);

            if append_to_last {
                append_to_last = false;
                let last_line = self.lines.last_mut().expect("can't append to nothing");
                let last_index = last_line.index_in_buffer();

                let slice = &self.raw_buffer[last_index..index + trunc.len()];
                // info!("AAAFG: {:?}", slice);
                let mut line = match slice.into_line_lossy(None, Style::new()) {
                    Ok(line) => line,
                    Err(_) => {
                        error!("ansi-to-tui failed to parse input! Using unstyled text.");
                        Line::from(String::from_utf8_lossy(slice).to_string())
                    }
                };

                line.remove_unsavory_chars();

                // if is_line_styled(&line) {
                //     debug!("is styled!");
                //     // line.style = Style::new().red().slow_blink();
                // }
                last_line.value = line;
                last_line.update_line_height(
                    self.last_terminal_size.width,
                    self.state.timestamps_visible,
                );
            } else {
                let line = match trunc.into_line_lossy(None, Style::new()) {
                    Ok(line) => line,
                    Err(_) => {
                        error!("ansi-to-tui failed to parse input! Using unstyled text.");
                        Line::from(String::from_utf8_lossy(trunc).to_string())
                    }
                };

                // if is_line_styled(&line) {
                //     debug!("is styled!");
                //     // line.style = Style::new().red().slow_blink();
                // }
                self.lines.push(BufLine::new_with_line(
                    line,
                    index,
                    self.last_terminal_size.width,
                    self.state.timestamps_visible,
                ));
            };
        }

        self.last_line_completed = {
            // let last_line = self.lines.last().expect("expected at least one line");
            let expected_ending = self.line_ending.as_bytes();
            bytes.has_byte_suffix(expected_ending)
        };

        // let mut appending_to_last = !self.last_line_completed;
        // self.strings.iter_mut().for_each(|s| {
        // });

        // split_inclusive() or split()?

        // for line in converted.split(&self.line_ending) {
        //     // Removing messy-to-render characters, but they should be preserved in the raw_buffer for those who need to see them

        //     // TODO Replace tab with multiple spaces? (As \t causes smearing with ratatui currently.)
        //     let mut s = line.replace(&['\t', '\n', '\r'][..], "");

        //     // TODO ansi_to_tui lines??
        //     // Make sure to reuse raw buffer for when appending to a line
        //     // but might need to do line ending splits pre-string-conversion..?

        //     // TODO Filter out ASCII control characters (like terminal bell)?
        //     s.retain(|c| !c.is_control() && !c.is_ascii_control());

        //     // TODO UTF-8 multi byte preservation between \n's?
        //     // Since if I am getting only one byte per second or read, then `String::from_utf8_lossy` could fail extra for no reason.

        //     if appending_to_last {
        //         let line = self.lines.last_mut().expect("Promised line to append to");
        //         line.value.push_str(&s);
        //         line.redetermine_color();
        //         line.update_line_count(
        //             self.last_terminal_size.width,
        //             self.state.timestamps_visible,
        //         );
        //         appending_to_last = false;
        //     } else {
        //         self.lines.push(BufLine::new(
        //             s,
        //             self.raw_buffer.len().saturating_sub(1),
        //             self.last_terminal_size.width,
        //             self.state.timestamps_visible,
        //         ));
        //         // self.lines.push(Line::raw(line.to_owned()));
        //     }
        // }

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
    /// Updates each BufLine's render height with the new terminal width, returning the sum total at the end
    pub fn update_wrapped_line_heights(&mut self) -> usize {
        self.lines.iter_mut().fold(0, |total, l| {
            let new_height =
                l.update_line_height(self.last_terminal_size.width, self.state.timestamps_visible);

            total + new_height
        })
    }
    pub fn lines_iter(&self) -> (impl Iterator<Item = Line>, u16) {
        // TODO styling based on line prefix
        // or have BufLine.value be an enum for String/ratatui::Line
        // and then match against at in BufLine::as_line()
        let last_size = &self.last_terminal_size;
        let total_lines = self.line_count();
        let more_lines_than_height = total_lines > last_size.height as usize;

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
                    for (index, entries_lines) in
                        self.lines.iter().map(|l| l.get_line_height()).enumerate()
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
                        .lines
                        .iter()
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
            self.lines
                .iter()
                .skip(entries_to_skip)
                .take(entries_to_take)
                .map(|l| l.as_line(self.state.timestamps_visible)),
            wrapped_scroll,
        )

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
        self.lines.iter().map(|l| l.get_line_height()).sum()
    }

    /// Returns the total amount of lines that can be rendered,
    /// taking into account if text wrapping is enabled or not.
    pub fn line_count(&self) -> usize {
        if self.state.text_wrapping {
            self.wrapped_line_count()
        } else {
            self.lines.len()
        }
    }

    pub fn update_terminal_size(&mut self, whole_terminal_size: Size) {
        self.last_terminal_size = {
            let mut terminal_size = whole_terminal_size;
            // `2` is the lines from the repeating_pattern_widget and the input buffer.
            // Might need to make more dynamic later?
            terminal_size.height = terminal_size.height.saturating_sub(2);
            terminal_size
        };
        self.update_wrapped_line_heights();
        self.scroll_by(0);
    }

    // pub fn line_ending(&self) -> &str {
    //     &self.line_ending
    // }
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
) -> Option<impl Iterator<Item = (&'a [u8], &'a [u8])>> {
    assert!(!line_ending.is_empty(), "line_ending can't be empty");
    // TODO maybe do line ending splits at this level, so raw_buffer_index can be more accurate
    // https://docs.rs/memchr/latest/memchr/memmem/index.html

    let line_ending = line_ending.as_bytes();

    let mut line_ending_pos_iter = memchr::memmem::find_iter(bytes, line_ending)
        .map(|line_ending_index| (line_ending_index, false))
        .chain(std::iter::once((bytes.len(), true)))
        .peekable();

    if let Some((_, is_final_entry)) = line_ending_pos_iter.peek() {
        // No line endings were found, only iter item present is our extra chained "final item" indicator.
        // TODO, maybe skip this check and allow returning the single full string slice?
        if *is_final_entry {
            return None;
        }
    } else {
        unreachable!("Expected at least the final chained iter!");
    }

    let mut last_index = 0;

    let slices_iter = line_ending_pos_iter.map(move |(line_ending_index, is_final_entry)| {
        if is_final_entry {
            (
                &bytes[last_index..bytes.len()],
                &bytes[last_index..bytes.len()],
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
            )
        }
    });

    Some(slices_iter)

    // for line_ending_pos in line_ending_pos_iter {
    //     let line_ending_pos = line_ending_pos;
    //     let is_complete_line = bytes[last_index..line_ending_pos + self.line_ending.len()]
    //         .has_byte_suffix(self.line_ending.as_bytes());
    //     let slice = &bytes[last_index..line_ending_pos];

    //     append_to_self(slice, is_complete_line);

    //     // debug!(
    //     //     "[{last_index}..{line_ending_pos}]: {}",
    //     //     string.escape_debug()
    //     // );
    //     last_index = line_ending_pos + self.line_ending.len();
    // }

    // return std::iter::once(bytes);

    // let tab_spaces = " ".repeat(4);
    // let mut appending_to_last = if is_user_input {
    //     false
    // } else {
    //     !self.last_line_completed
    // };

    // let mut append_to_self = |slice, line_has_ending: bool| {
    //     let string = String::from_utf8_lossy(slice)
    //         .replace('\t', &tab_spaces)
    //         .replace(&['\n', '\r'][..], "");

    //     if appending_to_last {
    //         self.lines
    //             .last_mut()
    //             .expect("was promised line to append to")
    //             .append_bytes(slice);
    //     } else {
    //         let buf_line = BufLine::new(
    //             string.as_bytes(),
    //             0,
    //             self.last_terminal_size.width,
    //             self.state.timestamps_visible,
    //         );
    //         self.lines.push(buf_line);
    //     }
    //     if line_has_ending {
    //         appending_to_last = false;
    //     }
    // };

    // // In the case of an empty iterator where it found no line endings
    // // && !bytes.is_empty()
    // if last_index == 0 {
    //     append_to_self(bytes, false);
    // }
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

fn is_line_styled(line: &Line<'_>) -> bool {
    if line.style != Style::default() {
        debug!("line style was: {:?}", &line.style);
        return true;
    }
    for span in &line.spans {
        if span.style != Style::default() {
            debug!("span style was: {:?}", &span.style);
            return true;
        }
    }
    false
}

fn extract_line(text: Text<'_>) -> Option<Line<'_>> {
    if text.lines.is_empty() {
        return None;
    }
    let Text { lines, .. } = text;
    lines.into_iter().find(|l| !l.spans.is_empty())
}
