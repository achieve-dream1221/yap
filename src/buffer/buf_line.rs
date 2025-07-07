use std::borrow::Cow;

use bstr::ByteSlice;
use chrono::{DateTime, Local};
use compact_str::{CompactString, ToCompactString, format_compact};
use ratatui::{
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use ratatui_macros::{line, span};
use tracing::debug;

#[cfg(feature = "defmt")]
use defmt_parser::Level;

#[cfg(feature = "defmt")]
use crate::settings::Defmt;
use crate::{
    buffer::{LineEnding, RangeSlice},
    settings::Rendering,
    traits::{ByteSuffixCheck, FirstChars, LineHelpers},
};

#[derive(Debug, Clone)]
pub struct BufLine {
    pub(super) timestamp: DateTime<Local>,
    timestamp_str: CompactString,

    index_info: CompactString,

    pub(super) value: Line<'static>,

    /// How many vertical lines are needed in the terminal to fully show this line.
    // Truncated from usize, since even the ratatui sizes are capped there.
    rendered_line_height: u16,

    pub raw_buffer_index: usize,
    pub line_type: LineType,
    // #[cfg(feature = "defmt")]
    // defmt_level: Option<Level>,
}

#[derive(Clone, Copy)]
pub struct RenderSettings<'a> {
    pub rendering: &'a Rendering,
    #[cfg(feature = "defmt")]
    pub defmt: &'a Defmt,
}

// impl PartialEq for BufLine {
//     fn eq(&self, other: &Self) -> bool {
//         self.timestamp == other.timestamp
//     }
// }

// impl Eq for BufLine {}

// impl PartialOrd for BufLine {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         self.timestamp.partial_cmp(&other.timestamp)
//     }
// }

// impl Ord for BufLine {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         self.timestamp.cmp(&other.timestamp)
//     }
// }

// #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// pub(super) struct UserLine {
//     pub(super) is_bytes: bool,
//     pub(super) is_macro: bool,
// }

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LineType {
    Port {
        escaped_line_ending: Option<CompactString>,
    },
    User {
        is_bytes: bool,
        is_macro: bool,
        escaped_line_ending: Option<CompactString>,
        reloggable_raw: Vec<u8>,
    },
    #[cfg(feature = "defmt")]
    PortDefmt {
        level: Option<defmt_parser::Level>,
        location: Option<FrameLocation>,
        device_timestamp: Option<CompactString>,
        // /// Includes any potential prefix or terminator
        // total_frame_len: usize,
    },
}

#[cfg(feature = "defmt")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLocation {
    // Original type is u64 but I'm not storing that.
    line: u32,
    module: CompactString,
    file: CompactString,
}

#[cfg(feature = "defmt")]
impl From<&defmt_decoder::Location> for FrameLocation {
    fn from(value: &defmt_decoder::Location) -> Self {
        Self {
            line: value.line.try_into().expect("line larger than u32::MAX??"),
            module: value.module.to_compact_string(),
            file: value.file.display().to_compact_string(),
        }
    }
}

impl LineType {
    pub(super) fn is_bytes(&self) -> bool {
        match *self {
            LineType::User { is_bytes, .. } => is_bytes,
            _ => false,
        }
    }

    pub(super) fn is_macro(&self) -> bool {
        match *self {
            LineType::User { is_macro, .. } => is_macro,
            _ => false,
        }
    }
}

pub struct BufLineKit<'a> {
    pub full_range_slice: RangeSlice<'a>,
    pub area_width: u16,
    pub render: RenderSettings<'a>,
    pub timestamp: DateTime<Local>,
}

// Many changes needed, esp. in regards to current app-state things (index, width, color, showing timestamp)
impl BufLine {
    fn new(mut line: Line<'static>, kit: BufLineKit, line_type: LineType) -> Self {
        let time_format = "[%H:%M:%S%.3f] ";

        line.remove_unsavory_chars();

        let index_info = make_index_info(&kit.full_range_slice);

        let timestamp = kit.timestamp;

        let mut bufline = Self {
            timestamp_str: timestamp.format(time_format).to_compact_string(),
            timestamp,
            index_info,
            value: line,
            raw_buffer_index: kit.full_range_slice.range.start,
            rendered_line_height: 0,
            line_type,
        };
        // bufline.populate_line_ending(raw_value, line_ending);
        bufline.update_line_height(kit.area_width, kit.render);
        bufline
    }
    pub fn port_text_line(line: Line<'static>, kit: BufLineKit, line_ending: &LineEnding) -> Self {
        let line_type = LineType::Port {
            escaped_line_ending: line_ending.escaped_from(kit.full_range_slice.slice),
        };

        Self::new(
            line, kit, // line_ending,
            line_type,
        )
    }
    #[cfg(feature = "defmt")]
    pub fn port_defmt_line(
        line: Line<'static>,
        kit: BufLineKit,
        level: Option<defmt_parser::Level>,
        device_timestamp: Option<&dyn std::fmt::Display>,
        location: Option<FrameLocation>,
    ) -> Self {
        let line_type = LineType::PortDefmt {
            level,
            device_timestamp: device_timestamp.map(|ts| format_compact!("[{ts}] ")),
            location,
        };

        Self::new(
            line, kit, // line_ending,
            line_type,
        )
    }
    pub fn user_line(
        line: Line<'static>,
        kit: BufLineKit,
        line_ending: &LineEnding,
        is_bytes: bool,
        is_macro: bool,
        reloggable_raw: &[u8],
    ) -> Self {
        let line_type = LineType::User {
            is_bytes,
            is_macro,
            reloggable_raw: reloggable_raw.to_vec(),
            escaped_line_ending: line_ending.escaped_from(reloggable_raw),
        };

        Self::new(line, kit, line_type)
    }

    pub fn update_line(&mut self, line: Line<'static>, kit: BufLineKit, line_ending: &LineEnding) {
        assert_eq!(
            self.line_type,
            LineType::Port {
                escaped_line_ending: None
            }
        );

        self.index_info = make_index_info(&kit.full_range_slice);

        self.value = line;
        self.value.remove_unsavory_chars();

        let LineType::Port {
            escaped_line_ending,
        } = &mut self.line_type
        else {
            unreachable!();
        };

        if let Some(escaped) = line_ending.escaped_from(&kit.full_range_slice.slice) {
            _ = escaped_line_ending.insert(escaped);
        };

        // self.populate_line_ending(full_line_slice, line_ending);

        self.update_line_height(kit.area_width, kit.render);
    }

    pub fn update_line_height(&mut self, area_width: u16, rendering: RenderSettings) -> usize {
        let para = Paragraph::new(self.as_line(rendering)).wrap(Wrap { trim: false });
        // TODO make the sub 1 for margin/scrollbar more sane/clear
        // Paragraph::line_count comes from an unstable ratatui feature (unstable-rendered-line-info)
        // which may be changed/removed in the future. If so, I'll need to roll my own wrapping/find someone's to steal.
        let height = para.line_count(area_width.saturating_sub(1));
        self.rendered_line_height = height as u16;
        height
    }

    pub fn get_line_height(&self) -> u16 {
        self.rendered_line_height
    }

    /// Returns an owned `Line` that borrows from the current line's spans.
    pub fn as_line(&self, rendering: RenderSettings) -> Line {
        let borrowed_spans = self.value.borrowed_spans_iter();

        let dark_gray = Style::new().dark_gray();

        let indices_and_len = std::iter::once(Span::styled(
            Cow::Borrowed(self.index_info.as_ref()),
            dark_gray,
        ))
        .filter(|_| rendering.rendering.show_indices);

        let timestamp = std::iter::once(Span::styled(
            Cow::Borrowed(self.timestamp_str.as_ref()),
            dark_gray,
        ))
        .filter(|_| rendering.rendering.timestamps);

        #[cfg(feature = "defmt")]
        let defmt_device_timestamp = std::iter::once(&self.line_type).filter_map(|lt| match lt {
            _ if !rendering.defmt.device_timestamp => None,
            LineType::PortDefmt {
                device_timestamp: Some(device_timestamp),
                ..
            } => Some(Span::styled(device_timestamp, dark_gray)),
            _ => None,
        });

        #[cfg(feature = "defmt")]
        let defmt_level = std::iter::once(&self.line_type)
            .filter_map(|lt| match lt {
                LineType::PortDefmt { level, .. } => {
                    Some(super::tui::defmt::defmt_level_bracketed(*level))
                }
                _ => None,
            })
            .flatten();

        // #[cfg(feature = "defmt")]
        // let defmt_location = std::iter::once(&self.line_type).filter_map(|lt| match lt {
        //     LineType::PortDefmt {
        //         location: Some(FrameLocation { line, module, file }),
        //         ..
        //     } => Some(Span::styled(
        //         format!(" {module} @ {file}:{line}"),
        //         Style::new().dark_gray(),
        //     )),
        //     _ => None,
        // });

        fn shorten_module_path(full_module_path: &str) -> &str {
            full_module_path
                .split("::")
                .last()
                .unwrap_or(full_module_path)
        }

        fn shorten_file_path(full_file_path: &str) -> &str {
            full_file_path
                .split(&['/', '\\'])
                .last()
                .unwrap_or(full_file_path)
        }

        #[cfg(feature = "defmt")]
        let defmt_location = std::iter::once(&self.line_type).filter_map(|lt| match lt {
            LineType::PortDefmt {
                location:
                    Some(FrameLocation {
                        line: defmt_line_num,
                        module: defmt_module,
                        file: defmt_file,
                    }),
                ..
            } => {
                use crate::settings::DefmtLocation;

                let RenderSettings { rendering, defmt } = rendering;

                let module = &defmt.show_module;
                let file = &defmt.show_file;
                let line_num = defmt.show_line_number;

                match (module, file, line_num) {
                    (DefmtLocation::Hidden, DefmtLocation::Hidden, false) => return None,
                    _ => (),
                };

                let module_file_seperator =
                    if !module.is_hidden() && (!file.is_hidden() || line_num) {
                        " @ "
                    } else {
                        ""
                    };
                let file_line_seperator = if !file.is_hidden() && line_num {
                    ":"
                } else {
                    ""
                };

                let module = match module {
                    DefmtLocation::Hidden => "",
                    DefmtLocation::Shortened => shorten_module_path(defmt_module),
                    DefmtLocation::Full => defmt_module,
                };
                let file = match file {
                    DefmtLocation::Hidden => "",
                    DefmtLocation::Shortened => shorten_file_path(defmt_file),
                    DefmtLocation::Full => defmt_file,
                };
                let line_num = if line_num {
                    Cow::Owned(defmt_line_num.to_string())
                } else {
                    Cow::Borrowed("")
                };

                Some(Span::styled(
                    format!(
                        " {module}{module_file_seperator}{file}{file_line_seperator}{line_num}"
                    ),
                    Style::new().dark_gray(),
                ))
                // todo!()
            }
            _ => None,
        });

        let line_ending = std::iter::once(&self.line_type).filter_map(|lt| match lt {
            _ if !rendering.rendering.show_line_ending => None,
            LineType::Port {
                escaped_line_ending: Some(line_ending),
            } => Some(Span::styled(Cow::Borrowed(line_ending.as_str()), dark_gray)),
            LineType::Port {
                escaped_line_ending: None,
            } => None,
            LineType::User {
                escaped_line_ending: Some(line_ending),
                ..
            } => Some(Span::styled(Cow::Borrowed(line_ending.as_str()), dark_gray)),
            LineType::User {
                escaped_line_ending: None,
                ..
            } => None,
            #[cfg(feature = "defmt")]
            LineType::PortDefmt { .. } => None,
        });

        let spans = timestamp;

        #[cfg(feature = "defmt")]
        let spans = spans.chain(defmt_device_timestamp);

        let spans = spans.chain(indices_and_len);

        #[cfg(feature = "defmt")]
        let spans = spans.chain(defmt_level);

        let spans = spans.chain(borrowed_spans).chain(line_ending);

        #[cfg(feature = "defmt")]
        let spans = spans.chain(defmt_location);

        Line::from_iter(spans)
    }

    pub fn index_in_buffer(&self) -> usize {
        self.raw_buffer_index
    }
}

fn make_index_info(
    range: &RangeSlice,
    // line_type: &LineType,
) -> CompactString {
    // if let LineType::User { .. } = line_type {
    //     format_compact!(
    //         "({start:06}->{end:06}, {len:3}) ",
    //         start = start_index,
    //         end = start_index + full_line_slice.len(),
    //         len = full_line_slice.len(),
    //     )
    // } else {
    let start = range.range.start;
    let end = range.range.end;
    let len = range.slice.len();
    debug_assert_eq!(end - start, len);
    format_compact!("({start:06}..{end:06}, {len:3}) ")
    // }
}
