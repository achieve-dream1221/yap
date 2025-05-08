use std::borrow::Borrow;

use compact_str::CompactString;
use crokey::KeyCombination;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};
use ratatui_macros::vertical;
use tui_input::Input;

use crate::traits::LineHelpers;

use super::{Macro, MacroRef};

#[derive(Debug, Default)]
pub struct MacroEditing {
    // if is_some, is editing an existing macro
    pub inner_ref: Option<MacroRef>,
    pub input: Input,

    pub selected_element: MeowOptions,

    // Not using an enum for these to allow going from bytes->lossy string->bytes without losing anything.
    scratch_macro: Macro,
    scratch_string: CompactString,
    scratch_bytes: Vec<u8>,
    scratch_keybind: Option<KeyCombination>,
}

impl MacroEditing {
    pub fn new() -> Self {
        Self {
            inner_ref: None,
            ..Default::default()
        }
    }
    pub fn editing(macro_ref: MacroRef) -> Self {
        Self {
            inner_ref: Some(macro_ref),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, int_enum::IntEnum, enum_rotate::EnumRotate)]
#[repr(u8)]
pub enum MeowOptions {
    #[default]
    Title,
    Category,
    ContentType,
    Content,
    Keybind,
    Finish,
}

// impl Widget for &MacroEditing {
//     fn render(self, screen: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
//     where
//         Self: Sized,
//     {

impl MacroEditing {
    pub fn render(&self, frame: &mut Frame, screen: Rect) {
        let title = if self.inner_ref.is_some() {
            "Edit Macro"
        } else {
            "New Macro"
        };

        let outer = Block::bordered()
            .border_style(Style::from(Color::LightGreen))
            .title_alignment(Alignment::Center)
            .title_style(Style::new().reset())
            .title_top(title);

        let min_width = 30;
        let min_height = 25;

        let rect = Rect {
            height: (min_height as u16).min(screen.height),
            width: (min_width as u16).min(screen.width),
            x: (screen.width.saturating_sub(min_width as u16)) / 2,
            y: (screen.height.saturating_sub(min_height as u16)) / 2,
        };
        frame.render_widget(Clear, rect);
        frame.render_widget(&outer, rect);

        let block = Block::new()
            // .border_style(Style::from(Color::LightGreen))
            .borders(Borders::TOP);

        let inner_area = outer.inner(rect);

        let [
            title_desc,
            title_entry,
            mut _sep_line,
            category_desc,
            category_entry,
            mut _sep_line2,
            content_type_desc,
            content_type,
            mut _sep_line3,
            content_entry_desc,
            content_entry,
            mut _sep_line4,
            keybind_desc,
            keybind_entry,
            mut _sep_line_spacing,
            mut _sep_line5,
            finish_button,
        ] = vertical![
            // Title
            ==1, ==1,
            // Sep
            ==1,
            // Category
            ==1, ==1,
            // Sep
            ==1,
            // Content Type
            ==1, ==1,
            // Sep
            ==1,
            // Content Entry
            ==1, ==1,
            // Sep
            ==1,
            // Keybind
            ==1, ==1,
            // Sep + Spacing
            *=1,
            // Sep
            ==1,
            // Finish
            ==1
        ]
        .areas(inner_area);

        let sep_lines = [
            &mut _sep_line,
            &mut _sep_line2,
            &mut _sep_line3,
            &mut _sep_line4,
            &mut _sep_line_spacing,
            &mut _sep_line5,
        ];

        for sep in sep_lines {
            sep.width = sep.width.saturating_sub(2);
            sep.x += 1;
        }

        let meow = |position: MeowOptions,
                    selected: MeowOptions,
                    area: Rect|
         -> (Paragraph<'_>, Option<(u16, u16)>) {
            let is_selected = position == selected;
            let orig_text: &str = match position {
                MeowOptions::Category => self
                    .scratch_macro
                    .category
                    .as_ref()
                    .map(CompactString::as_str)
                    .map(|s| if s.is_empty() { " " } else { s })
                    .unwrap_or(" "),
                MeowOptions::Content => "grrrr",
                MeowOptions::Title if self.scratch_macro.title.is_empty() => " ",
                MeowOptions::Title => self.scratch_macro.title.as_str(),

                MeowOptions::Keybind | MeowOptions::Finish | MeowOptions::ContentType => {
                    unreachable!();
                }
            };

            // match current {
            //     MeowOptions::Finish => "",
            //     MeowOptions::Category => "",
            //     MeowOptions::Content => "",
            //     MeowOptions::ContentType => "",
            //     MeowOptions::Keybind => "",
            //     MeowOptions::Title => "",
            // }

            if is_selected {
                let width = inner_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge
                let scroll = self.input.visual_scroll(width as usize);
                let input_text = Paragraph::new(self.input.value())
                    .reversed()
                    .scroll((0, scroll as u16));

                // if should_position_cursor {
                // frame.set_cursor_position((
                // ));
                // }
                // Line::raw(orig_text).reversed()
                (
                    input_text,
                    Some((
                        // Put cursor past the end of the input text
                        area.x + ((self.input.visual_cursor()).max(scroll) - scroll) as u16,
                        area.y,
                    )),
                )
            } else {
                (Paragraph::new(Line::raw(orig_text).centered()), None)
            }
        };

        let mut cursor_pos = None;

        frame.render_widget(Line::raw("Title:").centered(), title_desc);
        let content = match meow(MeowOptions::Title, self.selected_element, title_entry) {
            (text, Some(cursor)) => {
                cursor_pos.replace(cursor);
                text
            }
            (text, None) => text,
        };
        frame.render_widget(content, title_entry);
        // frame.render_widget(Line::raw("[     ]").centered(), title_entry);
        frame.render_widget(&block, _sep_line);
        frame.render_widget(Line::raw("Category:").centered(), category_desc);
        let content = match meow(MeowOptions::Category, self.selected_element, category_entry) {
            (text, Some(cursor)) => {
                cursor_pos.replace(cursor);
                text
            }
            (text, None) => text,
        };
        frame.render_widget(content, category_entry);
        // frame.render_widget(Line::raw("[     ]").centered(), category_entry);
        frame.render_widget(&block, _sep_line2);
        frame.render_widget(Line::raw("Macro Type:").centered(), content_type_desc);
        frame.render_widget(Line::raw("[Text] Bytes").centered(), content_type);
        frame.render_widget(&block, _sep_line3);
        frame.render_widget(Line::raw("Content:").centered(), content_entry_desc);
        let content = match meow(MeowOptions::Content, self.selected_element, content_entry) {
            (text, Some(cursor)) => {
                cursor_pos.replace(cursor);
                text
            }
            (text, None) => text,
        };
        frame.render_widget(content, content_entry);
        // frame.render_widget(Line::raw("Meow").centered(), content_entry);
        frame.render_widget(&block, _sep_line4);
        frame.render_widget(Line::raw("Keybind:").centered(), keybind_desc);
        frame.render_widget(Line::raw("[Ctrl-m]").centered(), keybind_entry);
        frame.render_widget(&block, _sep_line_spacing);
        frame.render_widget(&block, _sep_line5);
        frame.render_widget(Line::raw("[Finish]").centered(), finish_button);

        let hint_area = {
            let mut area = rect.clone();
            area.y = area.bottom().saturating_sub(1);
            area.height = 1;
            area
        };

        frame.render_widget(
            Line::raw("Esc: Cancel")
                .all_spans_styled(Style::new().dark_gray())
                .centered(),
            hint_area,
        );

        if let Some(pos) = cursor_pos {
            frame.set_cursor_position(pos);
        }
    }
}

// frame.render_stateful_widget(prompt, rect, state);
// }
// }

pub struct MeowState {}

// pub fn render_macro_edit(frame: &mut Frame, given_area: Rect) {}

pub fn render_keybind_record() {
    // Recording keybind for {name}
    // Del/Backspace: Clear | Enter : Save
    // [Ctrl+Q]
    // Esc: Cancel
    // Style: Color::LightGreen?
}
