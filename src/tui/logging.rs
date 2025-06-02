use std::{borrow::Cow, path::PathBuf};

use compact_str::CompactString;
use ratatui::{
    prelude::*,
    widgets::{Block, Clear, Gauge, Row, Table, TableState},
};
use ratatui_macros::{line, vertical};
use tracing::debug;

use crate::traits::{LastIndex, LineHelpers};

use std::collections::BTreeMap;

use serde::Deserialize;

use super::centered_rect_size;

// // TODO move file stuff out of TUI module

// pub struct LoggingTui {
//     logging_active: bool,
//     settings: Table<'static>,
// }

// impl LoggingTui {
//     pub fn new(logging_active: bool, settings: Table<'static>) -> Self {
//         Self {
//             logging_active,
//             settings,
//         }
//     }
// }

// impl Widget for LoggingTui {
//     fn render(self, area: Rect, buf: &mut Buffer)
//     where
//         Self: Sized,
//     {

//     }
// }

// pub const ESPFLASH_BUTTON_COUNT: usize = 4;

pub fn toggle_logging_button(table_state: &mut TableState, logging_active: bool) -> Table<'static> {
    table_state.select_first_column();
    let selected_row_style = Style::new().reversed();
    let first_column_style = Style::new().reset();

    let toggle_text = if logging_active { "Stop!" } else { "Start!" };

    let rows: Vec<Row> = vec![Row::new([
        Text::raw("Start/Stop Logging to file(s) ").right_aligned(),
        Text::raw(toggle_text).centered().italic(),
    ])];

    let option_table = Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .column_highlight_style(first_column_style)
    .row_highlight_style(selected_row_style);

    option_table
}
