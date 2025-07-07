use std::{error::Error, fmt::Display};

use ratatui::widgets::Table;

pub use struct_table_derive::*;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ArrowKey {
    Up,
    Down,
    Left,
    Right,
}

impl std::fmt::Display for ArrowKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let key = match self {
            ArrowKey::Up => "Up",
            ArrowKey::Down => "Down",
            ArrowKey::Left => "Left",
            ArrowKey::Right => "Right",
        };
        write!(f, "{key}")
    }
}

#[derive(Debug)]
pub struct InvalidFieldIndex;

impl Display for InvalidFieldIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "given field index out of range")
    }
}

impl Error for InvalidFieldIndex {}

pub trait StructTable: ::core::marker::Sized + 'static {
    /// Returns `true` if the input caused a change in the struct.
    ///
    /// Returns an `Err` if a change was attempted at an invalid field index (>= field amount).
    fn handle_input(
        &mut self,
        input: ArrowKey,
        field_index: usize,
    ) -> Result<bool, InvalidFieldIndex>;
    fn as_table(&self) -> Table<'_>;
    const DOCSTRINGS: &'static [&'static str];
    const VISIBLE_FIELDS: usize;
}
