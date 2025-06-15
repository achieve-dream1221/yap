use ratatui::{
    prelude::*,
    widgets::{Row, Table},
};

pub fn toggle_logging_button(logging_active: bool) -> Table<'static> {
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
