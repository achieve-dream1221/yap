use std::borrow::Cow;

use ratatui::{
    prelude::*,
    text::Line,
    widgets::{Row, TableState},
};
use ratatui_macros::line;
use tracing::debug;

pub struct SingleLineSelector<'a> {
    items: Vec<Line<'a>>,
    max_line_chars: usize,
    next_symbol: Option<&'a str>,
    next_style: Style,
    prev_symbol: Option<&'a str>,
    prev_style: Style,
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
            .map(line_chars)
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
            // text_style: Style::new(),
        }
    }
    pub fn items(&self) -> &Vec<Line<'_>> {
        &self.items
    }
}

impl StatefulWidget for SingleLineSelector<'_> {
    type State = SingleLineSelectorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.current_index >= self.items.len() {
            state.current_index = self.items.len().saturating_sub(1);
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

        let next_span = if state.current_index == self.items.len().saturating_sub(1) {
            Span::raw(" ")
        } else {
            Span::styled(
                self.next_symbol.unwrap_or(DEFAULT_NEXT_SYMBOL),
                self.next_style,
            )
        };

        // TODO do centering with this somehow
        // currently its "left-aligned", which is *fine*
        // but just looks ugly

        let deficit = self.max_line_chars.saturating_sub(line_chars(source_line));

        // debug!("{max_chars}");

        let borrowed_line = source_line.iter().map(|i| match &i.content {
            Cow::Owned(owned) => Cow::Borrowed(owned.as_str()),
            Cow::Borrowed(borrowed) => Cow::Borrowed(*borrowed),
        });

        // ugly but works for the moment
        let line = {
            once(prev_span)
                .chain(once(Span::raw(" ")))
                // not a fan of the deep clone for owned cows
                .chain(borrowed_line.map(Span::raw))
                .chain(repeat(Span::raw(" ")).take(deficit))
                .chain(once(Span::raw(" ")))
                .chain(once(next_span))
        };

        let line = if state.active {
            Line::from_iter(line.map(Span::reversed)).centered()
        } else {
            Line::from_iter(line).centered()
        };

        // if !state.active {
        //     line = line.styl
        // }

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

fn line_chars(l: &Line<'_>) -> usize {
    l.iter().map(|s| s.content.chars().count()).sum()
}

pub trait StateBottomed<T> {
    /// Returns true if the index of the given state is equal to or greater than the index of the final element.
    fn on_last(&self, slice: &[T]) -> bool;
}

impl<T> StateBottomed<T> for SingleLineSelectorState {
    fn on_last(&self, slice: &[T]) -> bool {
        self.current_index >= slice.len().saturating_sub(1)
    }
}

impl<T> StateBottomed<T> for TableState {
    fn on_last(&self, slice: &[T]) -> bool {
        match self.selected() {
            None => false,
            Some(index) => index >= slice.len().saturating_sub(1),
        }
    }
}
