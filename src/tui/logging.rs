use ratatui::{
    prelude::*,
    widgets::{Row, Table},
};

pub fn sync_logs_button() -> Table<'static> {
    let cell_highlight_style = Style::new().reversed().italic();

    let rows: Vec<Row> = vec![Row::new([
        Text::raw("Sync Buffer to File(s) ").right_aligned(),
        Text::raw("Sync!").centered().italic(),
    ])];

    Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .cell_highlight_style(cell_highlight_style)
}
