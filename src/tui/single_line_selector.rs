use std::iter::repeat_n;

use num_integer::Integer;
use ratatui::{prelude::*, text::Line, widgets::TableState};

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

pub struct SingleLineSelectorState {
    pub current_index: usize,
    pub active: bool,
}

// const DEFAULT_NEXT_SYMBOL: &str = ">";
// const DEFAULT_PREV_SYMBOL: &str = "<";
const DEFAULT_NEXT_SYMBOL: &str = "→";
const DEFAULT_PREV_SYMBOL: &str = "←";

impl<'a> SingleLineSelector<'a> {
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Line<'a>>,
    {
        let items: Vec<_> = items
            .into_iter()
            .map(Into::into)
            // .map(Line::centered)
            .collect();

        let max_line_chars: usize = items
            .iter()
            // Getting the sum of chars for each line
            .map(Line::width)
            // .flatten()
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

        let source_line = { self.items.get(state.current_index).unwrap() };

        use std::iter::{once, repeat};

        let prev_span = if state.current_index == 0 {
            Span::raw(" ")
        } else {
            Span::styled(
                self.prev_symbol.unwrap_or(DEFAULT_PREV_SYMBOL),
                self.prev_style,
            )
        };

        let next_span = if self.items.last_index_eq(state.current_index) {
            Span::raw(" ")
        } else {
            Span::styled(
                self.next_symbol.unwrap_or(DEFAULT_NEXT_SYMBOL),
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

        let space_iter = once(Span::raw(" ")).filter(|_| self.space_padding);

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

        // if !state.active {
        //     line = line.styl
        // }

        let reset_line = Line::from_iter(repeat_n(Span::raw(" ").reset(), line.width())).centered();
        reset_line.render(area, buf);
        line.render(area, buf);
        // if state.content_length == 0 || self.track_length_excluding_arrow_heads(area) == 0 {
        //     return;
        // }
        // // https://docs.rs/ratatui/latest/src/ratatui/widgets/scrollbar.rs.html#150-159
        // // https://docs.rs/ratatui/latest/src/ratatui/widgets/table/table.rs.html#238-277
        // let mut bar = self.bar_symbols(area, state);
        // let area = self.scollbar_area(area);
        // for x in area.left()..area.right() {
        //     for y in area.top()..area.bottom() {
        //         if let Some(Some((symbol, style))) = bar.next() {
        //             buf.set_string(x, y, symbol, style);
        //         }
        //     }
        // }
    }
}

impl SingleLineSelector<'_> {
    fn selector_symbols(
        &self,
        area: Rect,
        state: &SingleLineSelectorState,
    ) -> impl Iterator<Item = u8> {
        std::iter::once(0)
    }
}

impl SingleLineSelectorState {
    pub fn new() -> Self {
        SingleLineSelectorState {
            current_index: 0,
            active: false,
        }
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

pub trait StateBottomed<T> {
    /// Returns true if the index of the given state is equal to or greater than the index of the final element.
    fn on_last(&self, slice: &[T]) -> bool;
}

impl<T> StateBottomed<T> for SingleLineSelectorState {
    fn on_last(&self, slice: &[T]) -> bool {
        self.current_index >= slice.last_index()
    }
}

impl<T> StateBottomed<T> for TableState {
    fn on_last(&self, slice: &[T]) -> bool {
        match self.selected() {
            None => false,
            Some(index) => index >= slice.last_index(),
        }
    }
}
