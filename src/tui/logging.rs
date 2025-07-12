use ratatui::{
    prelude::*,
    widgets::{Row, Table},
};

pub fn sync_logs_button() -> Table<'static> {
    let selected_row_style = Style::new().reversed();
    let first_column_style = Style::new().reset();

    let rows: Vec<Row> = vec![Row::new([
        Text::raw("Sync Buffer to File(s) ").right_aligned(),
        Text::raw("Sync!").centered().italic(),
    ])];

    Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .column_highlight_style(first_column_style)
    .row_highlight_style(selected_row_style)
}
