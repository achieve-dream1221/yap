//! The widget responsible for the `< ENTRY >` UI elements.

use num_integer::Integer;
use ratatui::{prelude::*, text::Line};

use crate::traits::{LastIndex, LineHelpers};

pub struct SingleLineSelector<'a> {
    items: Vec<Line<'a>>,
    max_line_chars: usize,
    next_symbol: Option<&'a str>,
    next_style: Style,
    prev_symbol: Option<&'a str>,
    prev_style: Style,
    space_padding: bool,
    // text_style: Style,
}

#[derive(Default)]
pub struct SingleLineSelectorState {
    pub current_index: usize,
    pub active: bool,
}

#[cfg(unix)]
pub fn default_next_symbol() -> &'static str {
    "→"
}
#[cfg(unix)]
pub fn default_prev_symbol() -> &'static str {
    "←"
}

// Ughhhhhhh.
// So, on some codepages on Windows (this was encountered on the Japanese codepage, 932),
// the arrow symbols from the extended graphics page, are _multi-cell_ characters.
//
// Now this normally wouldn't be a concern, but some older versions of
// `conhost.exe` (CMD and PowerShell's shared backend!) in some older versions of Windows (but as recent as Windows 10!)
// don't know how to handle this character properly and will crash with a stack overflow exception (0xc0000409).
// Newer builds of `conhost` than the one in Windows 10 (as of writing) do not seem to suffer from this bug,
// likely fixed around this time: https://github.com/microsoft/terminal/pull/4415
//
// The standalone Windows Terminal is unaffected.
#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
static DEFAULT_NEXT_SYMBOL: OnceLock<&'static str> = OnceLock::new();
#[cfg(windows)]
static DEFAULT_PREV_SYMBOL: OnceLock<&'static str> = OnceLock::new();

#[cfg(windows)]
pub fn default_next_symbol() -> &'static str {
    DEFAULT_NEXT_SYMBOL.get_or_init(|| {
        let acp = unsafe { windows_sys::Win32::System::Console::GetConsoleCP() };
        // Only allowing the default English US codepages for the fancy arrows.
        // I think only 437 is possible in the _console_?
        // But just in case, also allowing 1252, which is the default US-EN Windows Shell codepage.
        if acp == 1252 || acp == 437 {
            "→"
        } else {
            ">"
        }
    })
}
#[cfg(windows)]
pub fn default_prev_symbol() -> &'static str {
    DEFAULT_PREV_SYMBOL.get_or_init(|| {
        let acp = unsafe { windows_sys::Win32::System::Console::GetConsoleCP() };
        if acp == 1252 || acp == 437 {
            "←"
        } else {
            "<"
        }
    })
}

impl<'a> SingleLineSelector<'a> {
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Line<'a>>,
    {
        let items: Vec<_> = items.into_iter().map(Into::into).collect();

        let max_line_chars: usize = items
            .iter()
            // Getting the sum of chars for each line
            .map(Line::width)
            .max()
            .unwrap_or_default();
        Self {
            items,
            max_line_chars,
            next_symbol: None,
            next_style: Style::new(),
            prev_symbol: None,
            prev_style: Style::new(),
            space_padding: false,
            // text_style: Style::new(),
        }
    }
    pub fn items(&self) -> &Vec<Line<'_>> {
        &self.items
    }
    pub fn with_next_symbol(mut self, symbol: &'a str) -> Self {
        self.next_symbol = Some(symbol);
        self
    }
    pub fn with_next_style(mut self, style: Style) -> Self {
        self.next_style = style;
        self
    }
    pub fn with_prev_symbol(mut self, symbol: &'a str) -> Self {
        self.prev_symbol = Some(symbol);
        self
    }
    pub fn with_prev_style(mut self, style: Style) -> Self {
        self.prev_style = style;
        self
    }
    pub fn with_space_padding(mut self, space_padding: bool) -> Self {
        self.space_padding = space_padding;
        self
    }
    pub fn max_chars(&self) -> usize {
        self.max_line_chars
    }
    pub fn size_hint(&mut self, new: usize) {
        self.max_line_chars = self.max_line_chars.max(new);
    }
    pub fn with_size_hint(mut self, new: usize) -> Self {
        self.max_line_chars = self.max_line_chars.max(new);
        self
    }
}

impl StatefulWidget for &SingleLineSelector<'_> {
    type State = SingleLineSelectorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if self.items.last_index_eq_or_under(state.current_index) {
            state.current_index = self.items.last_index();
        }

        let source_line = {
            self.items
                .get(state.current_index)
                .expect("index should've been validated")
        };

        use std::iter::{once, repeat_n};

        let prev_span = if state.current_index == 0 {
            Span::raw(" ")
        } else {
            Span::styled(
                self.prev_symbol.unwrap_or(default_prev_symbol()),
                self.prev_style,
            )
        };

        let next_span = if self.items.last_index_eq(state.current_index) {
            Span::raw(" ")
        } else {
            Span::styled(
                self.next_symbol.unwrap_or(default_next_symbol()),
                self.next_style,
            )
        };

        // + 2 is to ensure deficit always spawns *some* padding
        let deficit = (self.max_line_chars + 2).saturating_sub(source_line.width());

        let (padding, extra) = deficit.div_rem(&2);

        // debug!("{deficit} / 2 = {padding}, {extra}");

        let (left, right) = (padding, padding + extra);

        // debug!("{max_chars}");

        let borrowed_line = source_line.borrowed_spans_iter();

        let space_iter = self.space_padding.then_some(Span::raw(" ")).into_iter();

        // ugly but works for the moment
        let line = {
            space_iter
                .clone()
                .chain(once(prev_span))
                .chain(repeat_n(Span::raw(" "), left.max(1)))
                .chain(borrowed_line)
                .chain(repeat_n(Span::raw(" "), right.max(1)))
                .chain(once(next_span))
                .chain(space_iter)
        };

        let line = if state.active {
            Line::from_iter(line.map(Span::reversed)).centered()
        } else {
            Line::from_iter(line).centered()
        };

        let reset_line = Line::from_iter(repeat_n(Span::raw(" ").reset(), line.width())).centered();
        reset_line.render(area, buf);
        line.render(area, buf);
    }
}

impl SingleLineSelectorState {
    pub fn new() -> Self {
        SingleLineSelectorState::default()
    }
    pub fn with_selected(mut self, new_index: usize) -> Self {
        self.select(new_index);
        self
    }
    // pub fn with_active(mut self, active: bool) -> Self {
    //     self.active = active;
    //     self
    // }
    pub fn next(&mut self) -> usize {
        self.current_index = self.current_index.saturating_add(1);
        self.current_index
    }
    pub fn prev(&mut self) -> usize {
        self.current_index = self.current_index.saturating_sub(1);
        self.current_index
    }
    pub fn select(&mut self, new_index: usize) {
        self.current_index = new_index;
    }
}
