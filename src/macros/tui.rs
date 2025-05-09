use std::{
    borrow::{Borrow, Cow},
    default,
};

use bstr::{ByteSlice, ByteVec};
use compact_str::{CompactString, ToCompactString, format_compact};
use crokey::KeyCombination;
use itertools::Itertools;
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, Paragraph},
};
use ratatui_macros::{line, span, vertical};
use tui_input::Input;

use crate::{traits::LineHelpers, tui::centered_rect_size};

use super::{Macro, MacroContent, MacroRef};

#[derive(Debug, Default)]
pub struct MacroEditing {
    // if is_some, is editing an existing macro
    pub inner_ref: Option<MacroRef>,
    pub input: Input,

    pub selected_element: MacroEditSelected,

    // Not using an enum for these to allow going from bytes->lossy string->bytes without losing anything.
    pub scratch_macro: Macro,
    pub scratch_string: CompactString,
    pub scratch_bytes: Vec<u8>,
    is_hex_valid: bool,
    pub scratch_keybind: Option<KeyCombination>,
    pub recording: bool,
}

impl MacroEditing {
    pub fn new() -> Self {
        Self {
            inner_ref: None,
            ..Default::default()
        }
    }
    pub fn editing(macro_clone: Macro) -> Self {
        Self {
            inner_ref: Some(MacroRef::from(&macro_clone)),
            input: Input::from(macro_clone.title.as_str()),
            scratch_bytes: macro_clone
                .as_bytes()
                .map(Vec::from)
                .unwrap_or_else(|| Vec::new()),
            scratch_string: macro_clone
                .as_str()
                .map(CompactString::from)
                .unwrap_or_else(|| CompactString::new("")),
            scratch_macro: macro_clone,
            ..Default::default()
        }
    }
    pub fn content_input(&self) -> Option<Cow<'_, str>> {
        match self.scratch_macro.content {
            MacroContent::Empty => None,
            MacroContent::Text(_) if !self.scratch_string.is_empty() => {
                Some(self.scratch_string.as_str().into())
            }
            MacroContent::Bytes { .. } if !self.scratch_bytes.is_empty() => {
                let text: String = self
                    .scratch_bytes
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect();
                Some(text.into())
            }
            _ => None,
        }
    }
    pub fn consume_input(&mut self) {
        match self.scratch_macro.content {
            MacroContent::Empty | MacroContent::Text(_) => {
                self.scratch_string = self.input.value().into();
            }
            MacroContent::Bytes { .. } => {
                let Ok(bytes) = self.validate_input_bytes(true) else {
                    return;
                };
                self.scratch_bytes = bytes;
            }
        }
        self.input.reset();
    }
    pub fn swap_content_type(&mut self) {
        self.scratch_macro.content = match self.scratch_macro.content {
            MacroContent::Empty | MacroContent::Text(_) => {
                let unescaped = Vec::unescape_bytes(self.scratch_string.as_str());
                if unescaped != self.scratch_bytes {
                    self.scratch_bytes = unescaped;
                }
                MacroContent::Bytes {
                    content: vec![],
                    preview: String::new(),
                }
            }
            MacroContent::Bytes { .. } => {
                let escaped = self.scratch_bytes.escape_bytes().to_compact_string();
                if escaped != self.scratch_string {
                    self.scratch_string = escaped;
                }
                // if self.scratch_string.as_bytes() != self.scratch_bytes {
                //     self.scratch_string = self
                //         .scratch_bytes
                //         .iter()
                //         .map(|b| format_compact!("{b:02X}"))
                //         .join("")
                //         .into();
                // }
                MacroContent::Text(String::new())
            }
        };
        self.input = self
            .content_input()
            .map(|s| s.to_string())
            .unwrap_or("".to_string())
            .into();
    }
    pub fn validate_input_bytes(
        &mut self,
        best_attempt: bool,
    ) -> Result<Vec<u8>, hex::FromHexError> {
        let mut input = self.input.value().replace(' ', "");
        let mut result = hex::decode(&input);
        if !result.is_ok() && best_attempt {
            while !input.is_empty() {
                input.pop();
                result = hex::decode(&input);
                if result.is_ok() {
                    break;
                }
            }
        }
        self.is_hex_valid = result.is_ok();
        result
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, int_enum::IntEnum, enum_rotate::EnumRotate)]
#[repr(u8)]
pub enum MacroEditSelected {
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
            _sep_line,
            category_desc,
            category_entry,
            _sep_line2,
            content_type_desc,
            content_type,
            _sep_line3,
            content_entry_desc,
            content_entry,
            _sep_line4,
            keybind_desc,
            keybind_entry,
            _sep_line_spacing,
            _sep_line5,
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

        // let sep_lines = [
        //     &mut _sep_line,
        //     &mut _sep_line2,
        //     &mut _sep_line3,
        //     &mut _sep_line4,
        //     &mut _sep_line_spacing,
        //     &mut _sep_line5,
        // ];

        // for sep in sep_lines {
        //     sep.width = sep.width.saturating_sub(2);
        //     sep.x += 1;
        // }

        // Returns styled scrolled input line with cursor position, or static line if own position not selected.
        let input_or_line = |position: MacroEditSelected,
                             selected: MacroEditSelected,
                             area: Rect|
         -> (Paragraph<'_>, Option<(u16, u16)>) {
            let is_selected = position == selected;
            let orig_text: Cow<'_, str> = match position {
                MacroEditSelected::Category => Cow::Borrowed(
                    self.scratch_macro
                        .category
                        .as_ref()
                        .map(CompactString::as_str)
                        .map(|s| if s.is_empty() { " " } else { s })
                        .unwrap_or(" "),
                ),
                MacroEditSelected::Content => Cow::Owned(
                    self.content_input()
                        .map(|s| {
                            if s.is_empty() {
                                " ".to_owned()
                            } else {
                                s.into_owned()
                            }
                        })
                        .unwrap_or(" ".to_string()),
                ),
                MacroEditSelected::Title if self.scratch_macro.title.is_empty() => {
                    Cow::Borrowed(" ")
                }
                MacroEditSelected::Title => Cow::Borrowed(self.scratch_macro.title.as_str()),

                MacroEditSelected::Keybind
                | MacroEditSelected::Finish
                | MacroEditSelected::ContentType => {
                    unreachable!();
                }
            };

            if is_selected {
                let width = inner_area.width.max(1) - 1; // So the cursor doesn't bleed off the edge
                let scroll = self.input.visual_scroll(width as usize);
                let mut input_text = Paragraph::new(self.input.value())
                    .reversed()
                    .scroll((0, scroll as u16));

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
        let content =
            match input_or_line(MacroEditSelected::Title, self.selected_element, title_entry) {
                (text, Some(cursor)) => {
                    cursor_pos.replace(cursor);
                    text
                }
                (text, None) => text,
            };
        frame.render_widget(content, title_entry);
        // frame.render_widget(Line::raw("[     ]").centered(), title_entry);
        frame.render_widget(&block, _sep_line);
        frame.render_widget(Line::raw("Category (opt.):").centered(), category_desc);
        let content = match input_or_line(
            MacroEditSelected::Category,
            self.selected_element,
            category_entry,
        ) {
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

        let reversed = Style::new().reversed();

        let type_reversed = if matches!(self.selected_element, MacroEditSelected::ContentType) {
            reversed
        } else {
            Style::new()
        };
        let type_line = match &self.scratch_macro.content {
            MacroContent::Empty | MacroContent::Text(_) => {
                line![span!(type_reversed; "[Text]"), span!(" Bytes ")]
            }
            MacroContent::Bytes { .. } => line![span!(" Text "), span!(type_reversed; "[Bytes]")],
        };

        frame.render_widget(type_line.centered(), content_type);
        frame.render_widget(&block, _sep_line3);
        frame.render_widget(Line::raw("Content:").centered(), content_entry_desc);
        let content = match input_or_line(
            MacroEditSelected::Content,
            self.selected_element,
            content_entry,
        ) {
            (text, Some(cursor)) => {
                cursor_pos.replace(cursor);
                text
            }
            (text, None) => text,
        };
        let content_style = match (&self.selected_element, &self.scratch_macro.content) {
            (MacroEditSelected::Content, MacroContent::Bytes { .. }) => {
                if self.is_hex_valid {
                    Style::new().green().reversed().italic()
                } else {
                    Style::new().red().reversed().italic()
                }
            }
            (MacroEditSelected::Content, _) => Style::new().reversed(),
            (_, MacroContent::Bytes { .. }) => Style::new().italic(),
            _ => Style::new(),
        };
        frame.render_widget(content.style(content_style), content_entry);
        // frame.render_widget(Line::raw("Meow").centered(), content_entry);
        frame.render_widget(&block, _sep_line4);
        frame.render_widget(Line::raw("Keybind:").centered(), keybind_desc);
        frame.render_widget(&block, _sep_line_spacing);

        if matches!(self.selected_element, MacroEditSelected::Keybind) {
            // TODO single line selector here for several keybinds per macro?
            frame.render_widget(
                Line::raw("[Ctrl-m]")
                    // use normal .reversed() Line if no keybind is set?
                    .all_spans_styled(reversed)
                    .centered(),
                keybind_entry,
            );

            frame.render_widget(
                Line::raw("Enter: Edit")
                    .all_spans_styled(Style::new().dark_gray())
                    .centered(),
                _sep_line_spacing,
            );
        } else {
            frame.render_widget(Line::raw("[Ctrl-m]").centered(), keybind_entry);
        }

        frame.render_widget(&block, _sep_line5);

        frame.render_widget(
            Line::raw("[Finish]")
                .all_spans_styled(
                    if matches!(self.selected_element, MacroEditSelected::Finish) {
                        reversed
                    } else {
                        Style::new()
                    },
                )
                .centered(),
            finish_button,
        );

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

        // let recording_idle = matches!(self.recording, MacroRecording::Shown);
        // let recording_active = matches!(self.recording, MacroRecording::Recording);

        if self.recording {
            let block = Block::bordered().border_style(Style::new().red());

            let rect = centered_rect_size(
                Size {
                    width: 40,
                    height: 9,
                },
                screen,
            );

            frame.render_widget(Clear, rect);
            frame.render_widget(&block, rect);

            let inner_rect = block.inner(rect);

            let [
                recording_title,
                recording_name,
                _spacing,
                current_macro,
                _spacing2,
                controls_hint,
            ] = vertical![==1,==1,==1,==1,<=2,==1].areas(inner_rect);

            frame.render_widget(
                Line::raw("Recording keybind for:").centered(),
                recording_title,
            );
            frame.render_widget(
                Line::raw(self.scratch_macro.title.as_str())
                    .italic()
                    .centered(),
                recording_name,
            );
            frame.render_widget(
                Line::styled(
                    format!(
                        "{}",
                        self.scratch_keybind
                            .map(|kb| kb.to_compact_string())
                            .unwrap_or_else(|| "<Unbound>".to_compact_string())
                    ),
                    Style::new().light_blue(),
                )
                .centered(),
                current_macro,
            );
            frame.render_widget(
                Line::raw("Esc: Cancel | Del: Clear | Enter: Save"),
                controls_hint,
            );
        }

        // match self.recording {
        //     MacroRecording::Hidden => (),
        //     MacroRecording::Shown => {},
        //     MacroRecording::Recording =>{}
        // }
    }
    pub fn start_recording(&mut self) {
        assert_eq!(self.recording, false);
        self.recording = true;
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
