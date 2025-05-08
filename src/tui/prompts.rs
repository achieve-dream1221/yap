use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::{Style, Stylize},
    widgets::{Block, Borders, Clear, Row, Table, TableState},
};
use ratatui_macros::row;
use strum::{VariantArray, VariantNames};
use unicode_width::UnicodeWidthStr;

pub struct Prompt {}

pub struct PromptState {}

// #[derive(Debug, strum::VariantNames, num_enum::IntoPrimitive, num_enum::TryFromPrimitive)]
#[repr(u8)]
pub enum Test {
    Yes,
    No,
    Cancel,
}

// TODO think about way to do keyboard shortcuts with these
// https://docs.rs/strum_macros/latest/strum_macros/derive.EnumProperty.html
// #[derive(Debug, strum::VariantNames, num_enum::IntoPrimitive, num_enum::TryFromPrimitive)]
#[derive(Debug, strum::VariantNames, int_enum::IntEnum)]
#[repr(u8)]
pub enum DisconnectPrompt {
    Disconnect,
    #[strum(serialize = "Open Port Settings")]
    PortSettings,
    #[strum(serialize = "Exit App")]
    Exit,
    Cancel,
}
#[derive(Debug, strum::VariantNames, int_enum::IntEnum)]
#[repr(u8)]
pub enum DeleteMacroPrompt {
    Delete,
    Cancel,
}

pub trait PromptTable: VariantNames + Into<u8> + TryFrom<u8> {
    /// Returns a ratatui [Table] with static references to the names of each enum variant.
    ///
    /// Enum variant names can be overwritten with the attribute:
    /// ```
    /// # #[derive(strum::VariantNames)]
    /// # enum ExampleEnum {
    /// #[strum(serialize = "Exit App")]
    /// #     Meow,
    /// # }
    /// ```
    fn prompt_table() -> Table<'static> {
        let selected_style = Style::new().reversed();

        // Fully Qualified:
        // <Self as self::VariantNames>::VARIANTS
        // (needed if I ever want to add strum::VariantsArray since it uses the same array name as VariantNames)
        let rows: Vec<Row> = Self::VARIANTS.iter().map(|s| row![*s]).collect();

        let option_table = Table::new(rows, [Constraint::Percentage(100)])
            .row_highlight_style(selected_style)
            .highlight_symbol(">> ");

        option_table
    }
    fn prompt_table_block<'a>(
        top: Option<&'a str>,
        bottom: Option<&'a str>,
        border_style: Style,
    ) -> Table<'a> {
        let block = Block::default()
            .borders(Borders::ALL)
            .title_alignment(Alignment::Center)
            .border_style(border_style)
            .title_style(Style::new().reset());
        let block = if let Some(text) = top {
            block.title_top(text)
        } else {
            block
        };
        let block = if let Some(text) = bottom {
            block.title_top(text)
        } else {
            block
        };
        Self::prompt_table().block(block)
    }
    fn render_prompt_block_popup(
        top: Option<&str>,
        bottom: Option<&str>,
        border_style: Style,
        frame: &mut Frame,
        given_area: Rect,
        state: &mut TableState,
    ) {
        let prompt = Self::prompt_table_block(top, bottom, border_style);
        let top_width = top.map(str::len).unwrap_or_default();
        let bottom_width = bottom.map(str::len).unwrap_or_default();

        let min_width = top_width.max(bottom_width) + 16; // For margin of 8 on either side
        let min_height = Self::VARIANTS.len() + 2; // For block height
        let rect = Rect {
            height: (min_height as u16).min(given_area.height),
            width: (min_width as u16).min(given_area.width),
            x: (given_area.width.saturating_sub(min_width as u16)) / 2,
            y: (given_area.height.saturating_sub(min_height as u16)) / 2,
        };
        frame.render_widget(Clear, rect);
        frame.render_stateful_widget(prompt, rect, state);
    }
}

impl<T: VariantNames + Into<u8> + TryFrom<u8>> PromptTable for T {}

/// Returns a `Rect` with the provided percentage of the parent `Rect` and centered.
pub fn centered_rect(percent_x: u16, percent_y: u16, parent: Rect) -> Rect {
    let new_width = parent.width * percent_x / 100;
    let new_height = parent.height * percent_y / 100;
    Rect {
        width: new_width,
        height: new_height,
        x: (parent.width - new_width) / 2,
        y: (parent.height - new_height) / 2,
    }
}
