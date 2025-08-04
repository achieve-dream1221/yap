use std::str::FromStr;

use crokey::{KeyCombination, crossterm::event::KeyCode};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Rect},
    style::{Color, Style, Stylize},
    widgets::{Block, Borders, Clear, Row, Table, TableState},
};
use ratatui_macros::{row, span};
use strum::{EnumProperty, VariantArray, VariantNames};

// TODO another prop for alternate label?
// maybe just a single shared bool for all of them,
// and whichever is active is the one that gets flipped

pub trait PromptKeybind: Clone + strum::VariantArray + strum::EnumProperty {
    /// Take a crossterm KeyCode and return the variant that has a matching shortcut.
    fn from_key_code(value: KeyCode) -> Option<Self> {
        Self::VARIANTS
            .iter()
            .find(|v| {
                let Some(variant_binding) = v.get_str("keybind") else {
                    return false;
                };
                let variant_key_combo: KeyCombination = variant_binding
                    .parse()
                    .expect("hardcoded keycombo should be valid");
                match (value, variant_key_combo.as_letter()) {
                    (KeyCode::Char(given_char), Some(variant_char)) => given_char == variant_char,
                    _ => false,
                }
            })
            .cloned()
    }
}

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
/// For Terminal Screen only, when ESC pressed.
pub enum DisconnectPrompt {
    #[strum(props(keybind = "p"))]
    BackToPortSelection,
    #[strum(props(keybind = "d"))]
    DisconnectFromPort,
    #[strum(props(keybind = "s"))]
    OpenPortSettings,
    #[strum(props(keybind = "e"))]
    ExitApp,
    #[strum(props(keybind = "c"))]
    Cancel,
}

impl PromptKeybind for DisconnectPrompt {}

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
/// For Terminal Screen only, when ESC pressed _and_ reconnections are off/paused.
pub enum AttemptReconnectPrompt {
    #[strum(props(keybind = "p"))]
    BackToPortSelection,
    #[strum(props(keybind = "r"))]
    AttemptReconnect,
    #[strum(props(keybind = "s"))]
    OpenPortSettings,
    #[strum(props(keybind = "e"))]
    ExitApp,
    #[strum(props(keybind = "c"))]
    Cancel,
}

impl PromptKeybind for AttemptReconnectPrompt {}

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum IgnoreUsbDevicePrompt {
    #[strum(props(keybind = "y"))]
    #[strum(serialize = "Ignore by VID + PID")]
    IgnoreByVidPid,
    #[strum(serialize = "Ignore by VID + PID + Serial Number")]
    IgnoreByVidPidSerial,
    IgnoreByName,
    #[strum(props(keybind = "c"))]
    Cancel,
}

impl PromptKeybind for IgnoreUsbDevicePrompt {}

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum IgnorePortByNamePrompt {
    #[strum(props(keybind = "y"))]
    IgnoreByName,
    #[strum(props(keybind = "c"))]
    Cancel,
}

impl PromptKeybind for IgnorePortByNamePrompt {}

// #[derive(
//     Debug, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
// )]
// #[repr(u8)]
// pub enum DeleteMacroPrompt {
//     #[strum(props(color = "red"))]
//     Delete,
//     Cancel,
// }

pub trait PromptTable: VariantNames + VariantArray + EnumProperty + Into<u8> + TryFrom<u8> {
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
        let str_iter = <Self as VariantNames>::VARIANTS.iter();
        let variant_iter = <Self as VariantArray>::VARIANTS.iter();
        let rows: Vec<Row> = variant_iter
            .zip(str_iter)
            .map(|(variant, name)| {
                let style = if let Some(color) = variant.get_str("color") {
                    Style::from(Color::from_str(color).expect("hardcoded color should be valid"))
                } else {
                    Style::new()
                };
                row![span!(name)].style(style)
            })
            .collect();

        Table::new(rows, [Constraint::Percentage(100)])
            .row_highlight_style(selected_style)
            .highlight_symbol(">> ")
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
            block.title_bottom(text)
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
        let min_height = <Self as VariantNames>::VARIANTS.len() + 2; // For block height
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

// Blanket impl for compatible enums.
impl<T: VariantNames + VariantArray + EnumProperty + Into<u8> + TryFrom<u8>> PromptTable for T {}
