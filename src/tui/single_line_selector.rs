use ratatui::{prelude::*, widgets::Row};

pub struct SingleLineSelector<'a> {
    items: Vec<Line<'a>>,
    next_symbol: Option<&'a str>,
    next_style: Style,
    prev_symbol: Option<&'a str>,
    prev_style: Style,
    // text_style: Style,
}

pub struct SingleLineSelectorState {
    current_index: usize,
}

const DEFAULT_NEXT_SYMBOL: &str = ">";
const DEFAULT_PREV_SYMBOL: &str = "<";

impl<'a> SingleLineSelector<'a> {
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Line<'a>>,
    {
        let items = items.into_iter().map(Into::into).collect();
        Self {
            items,
            next_symbol: None,
            next_style: Style::new(),
            prev_symbol: None,
            prev_style: Style::new(),
            // text_style: Style::new(),
        }
    }
}

impl StatefulWidget for SingleLineSelector<'_> {
    type State = SingleLineSelectorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if state.content_length == 0 || self.track_length_excluding_arrow_heads(area) == 0 {
            return;
        }
        // https://docs.rs/ratatui/latest/src/ratatui/widgets/scrollbar.rs.html#150-159
        // https://docs.rs/ratatui/latest/src/ratatui/widgets/table/table.rs.html#238-277
        let mut bar = self.bar_symbols(area, state);
        let area = self.scollbar_area(area);
        for x in area.left()..area.right() {
            for y in area.top()..area.bottom() {
                if let Some(Some((symbol, style))) = bar.next() {
                    buf.set_string(x, y, symbol, style);
                }
            }
        }
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
