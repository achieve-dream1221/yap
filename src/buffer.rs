use chrono::{DateTime, Local};
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

use crate::app::{LINE_ENDINGS, LINE_ENDINGS_DEFAULT};

pub struct BufferState {
    pub text_wrapping: bool,
    pub timestamps_visible: bool,

    vert_scroll: usize,
    scrollbar_state: ScrollbarState,
    stuck_to_bottom: bool,
}

pub struct Buffer {
    raw_buffer: Vec<u8>,
    pub lines: Vec<BufLine>,
    // This technically *works* but I have issues with it
    // Namely that this is the size of the terminal
    // and not the actual buffer render area.
    last_terminal_size: Size,
    // pub color_rules
    pub state: BufferState,
}

#[derive(Debug)]
pub struct BufLine {
    value: String,
    // maybe? depends on whats easier to chain bytes from, for the hex view later
    // raw_value: Vec<u8>,
    rendered_line_count: usize,
    style: Option<Style>,
    // Might not be exactly accurate, but would be enough to place user input lines in proper space if needing to
    raw_buffer_index: usize,
    timestamp: String,
}
// Many changes needed, esp. in regards to current app-state things (index, width, color, showing timestamp)
impl BufLine {
    fn new(value: String, raw_buffer_index: usize, area_width: u16, show_timestamp: bool) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        let mut line = Self {
            value,
            raw_buffer_index,
            style: None,
            rendered_line_count: 0,
            timestamp: Local::now().format(time_format).to_string(),
        };
        line.update_line_count(area_width, show_timestamp);
        line.determine_color();
        line
    }
    fn completed(&self, line_ending: &str) -> bool {
        self.value.ends_with(line_ending)
    }
    fn update_line_count(&mut self, area_width: u16, show_timestamp: bool) {
        let para = Paragraph::new(self.as_line(show_timestamp)).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_count = height;
        // debug!("{self:?}");
    }
    fn determine_color(&mut self) {
        // Not sure if this is actually worth keeping, we'll see once I add proper custom rules.
        if self.style.is_some() {
            return;
        }
        // What do I pass into here?
        // The rules? Should it instead be an outside decider that supplies the color?

        if let Some(slice) = self.value.first_chars(5) {
            let mut style = Style::new();
            style = match slice {
                "USER>" => style.dark_gray(),
                "Got m" => style.blue(),
                "ID:0x" => style.green(),
                "Chan." => style.dark_gray(),
                "Mode:" => style.yellow(),
                "Power" => style.red(),
                _ => style,
            };

            if style != Style::new() {
                self.style = Some(style);
            }
        }
    }

    pub fn as_line(&self, show_timestamp: bool) -> Line {
        match (self.style, show_timestamp) {
            (Some(style), true) => line![
                span![Style::new().dark_gray(); &self.timestamp],
                span![style; &self.value]
            ],
            (None, true) => line![
                span![Style::new().dark_gray(); &self.timestamp],
                &self.value
            ],

            (Some(style), false) => Line::styled(&self.value, style),
            (None, false) => Line::raw(&self.value),
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
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
        }
    }
    // pub fn append_str(&mut self, str: &str) {
    // }

    // TODO also do append_user_bytes
    pub fn append_user_text(&mut self, text: &str) {
        // TODO dont use \n
        let value: String = format!("USER> {}\n", text.escape_debug());
        let line = BufLine::new(
            value,
            self.raw_buffer.len().saturating_sub(1),
            self.last_terminal_size.width,
            self.state.timestamps_visible,
        );
        self.lines.push(line);
    }

    // Forced to use Vec<u8> for now
    pub fn append_bytes(&mut self, bytes: &mut Vec<u8>) {
        let converted = String::from_utf8_lossy(&bytes).to_string();
        // TODO maybe do line ending splits at this level, so raw_buffer_index can be more accurate
        self.raw_buffer.append(bytes);

        let mut appending_to_last = self
            .lines
            .last()
            .map(|l| !l.completed(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]))
            .unwrap_or(false);
        // self.strings.iter_mut().for_each(|s| {
        // });

        // split_inclusive() or split()?
        for line in converted.split(LINE_ENDINGS[LINE_ENDINGS_DEFAULT]) {
            // Removing messy-to-render characters, but they should be preserved in the raw_buffer for those who need to see them

            // TODO Replace tab with multiple spaces? (As \t causes smearing with ratatui currently.)
            let mut s = line.replace(&['\t', '\n', '\r'][..], "");

            // TODO ansi_to_tui lines??

            // TODO Filter out ASCII control characters (like terminal bell)?
            s.retain(|c| !c.is_control() && !c.is_ascii_control());

            // TODO UTF-8 multi byte preservation between \n's?
            // Since if I am getting only one byte per second or read, then `String::from_utf8_lossy` could fail extra for no reason.

            if appending_to_last {
                let line = self.lines.last_mut().expect("Promised line to append to");
                line.value.push_str(&s);
                line.determine_color();
                line.update_line_count(
                    self.last_terminal_size.width,
                    self.state.timestamps_visible,
                );
                appending_to_last = false;
            } else {
                self.lines.push(BufLine::new(
                    s,
                    self.raw_buffer.len().saturating_sub(1),
                    self.last_terminal_size.width,
                    self.state.timestamps_visible,
                ));
                // self.lines.push(Line::raw(line.to_owned()));
            }
        }
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
    pub fn update_wrapped_line_count(&mut self) -> usize {
        self.lines.iter_mut().fold(0, |total, l| {
            l.update_line_count(self.last_terminal_size.width, self.state.timestamps_visible);

            total + l.rendered_line_count
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
                        self.lines.iter().map(|l| l.rendered_line_count).enumerate()
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
                        .map(|l| l.rendered_line_count)
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
    pub fn terminal_paragraph(&self, buffer_wrapping: bool) -> Paragraph<'_> {
        // let lines: Vec<_> = self
        //     .buffer
        //     .lines
        //     .iter()
        //     .map(|s| Cow::Borrowed(s.as_str()))
        //     .map(|c| if styled { coloring(c) } else { Line::raw(c) })
        //     .collect();
        let (lines_iter, vert_scroll) = self.lines_iter();
        let lines: Vec<_> = lines_iter.collect();
        // let vert_scroll = self.state.vert_scroll as u16;
        // let vert_scroll = if self.state.text_wrapping {
        //     self.state.vert_scroll as u16
        // } else {
        //     0
        // };
        let para = Paragraph::new(lines)
            .block(Block::new().borders(Borders::RIGHT))
            .scroll((vert_scroll, 0));
        if buffer_wrapping {
            // TODO make better logic for this where it takes in the current scroll,
            // only rendering the lines intersecting with the buffer's "window",
            // and handling scrolling itself.
            para.wrap(Wrap { trim: false })
        } else {
            para
        }
    }
    pub fn clear(&mut self) {
        self.lines.clear();
        self.raw_buffer.clear();
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
            // TODO do this proper when wrapping is toggleable
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
        self.lines.iter().map(|l| l.rendered_line_count).sum()
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
    }
}

// pub fn colored_line<'a, L: Into<Line<'a>>>(text: L) -> Line<'a> {
//     if text.
//     let line = Line::from(text);

// }

trait FirstChars {
    fn first_chars(&self, char_count: usize) -> Option<&str>;
}

impl FirstChars for str {
    fn first_chars(&self, desired: usize) -> Option<&str> {
        let char_count = self.chars().count();
        if char_count < desired {
            None
        } else if char_count == desired {
            Some(self)
        } else {
            let end = self
                .char_indices()
                .nth(desired)
                .map(|(i, _)| i)
                .expect("Not enough chars?");
            Some(&self[..end])
        }
    }
}

impl Widget for &mut Buffer {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        // TODO allow this to work
        // self.last_terminal_size = area.as_size();

        let para = self.terminal_paragraph(self.state.text_wrapping);
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
