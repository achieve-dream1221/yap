use std::path::PathBuf;

use compact_str::CompactString;
use espflash::{flasher::DeviceInfo, targets::Chip};
use fs_err as fs;
use ratatui::{
    prelude::*,
    widgets::{Block, Clear, Gauge, Row, Table, TableState},
};
use ratatui_macros::{line, vertical};
use tracing::debug;

use crate::{
    serial::esp::{EspFlashEvent, FlashProgress},
    traits::LineHelpers,
};

use std::collections::BTreeMap;

use serde::Deserialize;

use super::centered_rect_size;

#[derive(Debug, Clone)]
pub struct EspBins {
    pub name: CompactString,
    pub bins: Vec<(u32, PathBuf)>,
    pub upload_baud: Option<u32>,
    pub expected_chip: Option<Chip>,
    pub partition_table: Option<PathBuf>,
    pub no_skip: bool,
    pub no_verify: bool,
}

impl<'de> Deserialize<'de> for EspBins {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct EspBinsVisitor;

        impl<'de> Visitor<'de> for EspBinsVisitor {
            type Value = EspBins;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a table for EspBins in custom TOML format")
            }

            fn visit_map<A>(self, mut map: A) -> Result<EspBins, A::Error>
            where
                A: MapAccess<'de>,
            {
                use serde::de::Error;

                let mut name = None;
                let mut bins = Vec::new();
                let mut upload_baud = None;
                let mut expected_chip = None;
                let mut partition_table = None;
                let mut no_skip = false;
                let mut no_verify = false;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => {
                            let value: CompactString = map.next_value()?;
                            name = Some(value);
                        }
                        "upload_baud" => {
                            upload_baud = Some(map.next_value()?);
                        }
                        "chip" => {
                            let value: String = map.next_value()?;
                            expected_chip = Some(
                                value
                                    .parse::<Chip>()
                                    .map_err(|_| A::Error::custom("invalid chip type given"))?,
                            );
                        }
                        "partition_table" => {
                            let value: String = map.next_value()?;
                            partition_table = Some(PathBuf::from(value));
                        }
                        "no_verify" => {
                            no_verify = map.next_value()?;
                        }
                        "no_skip" => {
                            no_skip = map.next_value()?;
                        }
                        other if other.starts_with("0x") => {
                            let offset_num =
                                u32::from_str_radix(other.trim_start_matches("0x"), 16).map_err(
                                    |_| {
                                        A::Error::custom(format!("Invalid bin offset key: {other}"))
                                    },
                                )?;
                            let path_val: String = map.next_value()?;
                            bins.push((offset_num, PathBuf::from(path_val)));
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let name = name.ok_or_else(|| A::Error::missing_field("name"))?;
                // let upload_baud =
                //     upload_baud.ok_or_else(|| A::Error::missing_field("upload_baud"))?;

                // // bins in order of offset ascending
                // let mut bins_vec: Vec<(u32, PathBuf)> = bins.into_iter().collect();
                // bins_vec.sort_by_key(|(offset, _)| *offset);

                Ok(EspBins {
                    name,
                    bins,
                    upload_baud,
                    expected_chip,
                    partition_table,
                    no_skip,
                    no_verify,
                })
            }
        }

        deserializer.deserialize_map(EspBinsVisitor)
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct SerializedEspFiles {
    // elf: Vec<_>,
    bin: Vec<EspBins>,
}

#[derive(Debug, Default)]
pub struct Flashing {
    // current_file_name: CompactString,
    current_file_addr: u32,
    current_file_len: usize,
    current_progress: usize,
}

#[derive(Debug)]
pub enum EspPopup {
    Connecting,
    Connected { chip: CompactString },
    DeviceInfo(Text<'static>),
    Flashing(Flashing),
    Erasing { chip: CompactString },
}

#[derive(Debug)]
pub struct EspFlashState {
    popup: Option<EspPopup>,
    pub bins: Vec<EspBins>,
}

impl EspFlashState {
    pub fn new() -> Self {
        let meow = fs::read_to_string("../../esp_profiles.toml").unwrap();
        let SerializedEspFiles { bin } = toml::from_str(&meow).unwrap();
        debug!("{bin:#?}");
        Self {
            popup: None,
            bins: bin,
        }
    }
    pub fn consume_event(&mut self, event: EspFlashEvent) {
        match event {
            EspFlashEvent::DeviceInfo(info) => {
                debug!("{info:#?}");
                let DeviceInfo {
                    chip,
                    revision,
                    crystal_frequency,
                    flash_size,
                    features,
                    mac_address,
                } = info;

                let text = Text::from(vec![
                    line!["Chip: ", chip.to_string().to_uppercase()],
                    line!["Revision: ", format!("{revision:?}")],
                    line!["Crystal Osc. Frequency: ", format!("{crystal_frequency}")],
                    line!["Flash Size: ", format!("{flash_size}")],
                    line!["Features: ", format!("{features:?}")],
                    line!["MAC Address: ", format!("{mac_address}")],
                ]);

                self.popup = Some(EspPopup::DeviceInfo(text));
            }
            EspFlashEvent::FlashProgress(progress) => match progress {
                FlashProgress::Init { addr, size } => {
                    self.popup = Some(EspPopup::Flashing(Flashing {
                        current_file_addr: addr,
                        current_file_len: size,
                        current_progress: 0,
                    }));
                }
                FlashProgress::Progress(progress) => {
                    let Some(EspPopup::Flashing(flashing)) = self.popup.take() else {
                        unreachable!();
                    };
                    self.popup = Some(EspPopup::Flashing(Flashing {
                        current_progress: progress,
                        ..flashing
                    }));
                }
                FlashProgress::SegmentFinished => {
                    let Some(EspPopup::Flashing(flashing)) = self.popup.take() else {
                        unreachable!();
                    };
                    self.popup = Some(EspPopup::Flashing(Flashing {
                        current_progress: flashing.current_file_len,
                        ..flashing
                    }));
                    // TODO with last
                    ();
                }
            },
            EspFlashEvent::Connecting => self.popup = Some(EspPopup::Connecting),
            EspFlashEvent::Connected { chip } => self.popup = Some(EspPopup::Connected { chip }),
            EspFlashEvent::EraseStart { chip } => self.popup = Some(EspPopup::Erasing { chip }),
            EspFlashEvent::PortReturned => self.popup = None,
            _ => (),
        }
    }
    pub fn render_espflash(&self, frame: &mut Frame, screen: Rect) {
        let center_area = centered_rect_size(
            Size {
                width: 60,
                height: 8,
            },
            screen,
        );

        let Some(popup) = &self.popup else {
            return;
        };

        frame.render_widget(Clear, center_area);

        let border_color = match popup {
            EspPopup::Connected { .. } => Color::Green,
            EspPopup::Connecting { .. } => Color::Cyan,
            EspPopup::DeviceInfo { .. } => Color::LightGreen,
            EspPopup::Erasing { .. } => Color::Yellow,
            EspPopup::Flashing { .. } => Color::Blue,
        };

        let block_title = match popup {
            // EspPopup::Connected { .. } => " Connected! ",
            // EspPopup::Connecting { .. } => " Connecting... ",
            // EspPopup::Erasing { .. } => " Erasing... ",
            EspPopup::Erasing { .. } | EspPopup::Connected { .. } | EspPopup::Connecting => "",
            EspPopup::DeviceInfo { .. } => " Retrieved ESP Info ",
            EspPopup::Flashing { .. } => " Flashing... ",
        };

        let block = Block::bordered()
            .border_style(Style::from(border_color))
            .title_top(
                Line::raw(block_title)
                    .centered()
                    .all_spans_styled(Style::new().reset()),
            );

        frame.render_widget(&block, center_area);

        let inner_area = block.inner(center_area);

        let [
            title_area,
            body1_area,
            body2_area,
            chunks_text,
            which_bytes,
            progress_area,
        ] = vertical![==1,==1,*=1,==1,==1,==1].areas(inner_area);

        match popup {
            EspPopup::Flashing(Flashing {
                current_file_addr,
                current_file_len,
                current_progress,
            }) => {
                frame.render_widget(
                    line!["@ 0x", format!("{current_file_addr:06X}")].centered(),
                    title_area,
                );
                frame.render_widget(line!["Chunks: "].centered(), chunks_text);
                frame.render_widget(
                    line![format!("{current_progress} / {current_file_len}")].centered(),
                    which_bytes,
                );

                let percent = if *current_file_len == 0 {
                    0u16
                } else {
                    ((*current_progress as f64 / *current_file_len as f64) * 100.0).round() as u16
                };

                let progressbar = Gauge::default()
                    .gauge_style(Style::from(Color::Green))
                    .percent(percent);

                frame.render_widget(progressbar, progress_area);
            }
            EspPopup::Connecting => {
                frame.render_widget(
                    line!["Connecting to Espressif device..."].centered(),
                    body2_area,
                );
            }
            EspPopup::Connected { chip } => {
                frame.render_widget(line!["Connected to ", chip, "!"].centered(), body2_area);
            }
            EspPopup::Erasing { chip } => {
                frame.render_widget(line!["Erasing ", chip, "..."].centered(), body2_area);
            }
            EspPopup::DeviceInfo(text) => {
                frame.render_widget(text, inner_area);
            }
            _ => (),
        }
    }
}

pub fn meow(table_state: &mut TableState, bins: &Vec<EspBins>) -> Table<'static> {
    table_state.select_first_column();
    let selected_row_style = Style::new().reversed();
    let first_column_style = Style::new().reset();

    let mut rows: Vec<Row> = vec![
        Row::new([
            Text::raw("ESP->User Code  ").right_aligned(),
            Text::raw("Reboot!").centered().italic(),
        ]),
        Row::new([
            Text::raw("ESP->Bootloader ").right_aligned(),
            Text::raw("Reboot!").centered().italic(),
        ]),
        Row::new([
            Text::raw("ESP->Device Info").right_aligned(),
            Text::raw("Get!").centered().italic(),
        ]),
        Row::new([
            Text::raw("ESP->Erase Flash").right_aligned(),
            Text::raw("Erase!").centered().italic(),
        ]),
    ];

    for bin_profile in bins {
        rows.push(Row::new([
            Text::raw(format!("{} ", bin_profile.name)).right_aligned(),
            Text::raw("Flash!").centered().italic(),
        ]));
    }

    let option_table = Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .column_highlight_style(first_column_style)
    .row_highlight_style(selected_row_style);

    option_table
}
