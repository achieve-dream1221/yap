use std::{borrow::Cow, f32::consts::PI, path::PathBuf};

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
    serial::esp::{EspEvent, FlashProgress},
    traits::{LastIndex, LineHelpers},
};

use std::collections::BTreeMap;

use serde::Deserialize;

use super::centered_rect_size;

#[derive(Debug)]
pub enum EspProfile {
    Bins(EspBins),
    Elf(EspElf),
}

#[derive(Debug, Clone)]
pub struct EspBins {
    pub name: CompactString,
    pub bins: Vec<(u32, PathBuf)>,
    pub upload_baud: Option<u32>,
    pub expected_chip: Option<Chip>,
    // pub partition_table: Option<PathBuf>,
    pub no_skip: bool,
    pub no_verify: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct EspElf {
    pub name: CompactString,
    pub path: PathBuf,
    #[serde(default)]
    pub upload_baud: Option<u32>,
    #[serde(deserialize_with = "deserialize_chip")]
    #[serde(rename = "chip")]
    #[serde(default)]
    pub expected_chip: Option<Chip>,
    #[serde(default)]
    pub partition_table: Option<PathBuf>,
    #[serde(default)]
    pub bootloader: Option<PathBuf>,
    #[serde(default)]
    pub no_skip: bool,
    #[serde(default)]
    pub no_verify: bool,
    #[serde(default)]
    pub ram: bool,
}

fn deserialize_chip<'de, D>(deserializer: D) -> Result<Option<Chip>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    use std::str::FromStr;
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        Some(s) => Chip::from_str(&s)
            .map(Some)
            .map_err(|_| D::Error::custom("invalid chip type given")),
        None => Ok(None),
    }
}

impl<'de> Deserialize<'de> for EspBins {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
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
    elf: Vec<EspElf>,
    bin: Vec<EspBins>,
}

#[derive(Debug, Default)]
pub struct Flashing {
    // current_file_name: CompactString,
    chip: CompactString,
    current_file_addr: u32,
    current_file_len: usize,
    current_progress: usize,
}

#[derive(Debug)]
pub enum EspPopup {
    Connecting,
    Connected { chip: CompactString },
    DeviceInfo(Table<'static>),
    Flashing(Flashing),
    Erasing { chip: CompactString },
}

#[derive(Debug)]
pub struct EspFlashState {
    popup: Option<EspPopup>,
    bins: Vec<EspBins>,
    elfs: Vec<EspElf>,
    pub profiles_selected: bool,
}

impl EspFlashState {
    pub fn new() -> Self {
        let meow = fs::read_to_string("../../esp_profiles.toml").unwrap();
        let SerializedEspFiles { bin, elf } = toml::from_str(&meow).unwrap();
        debug!("{bin:#?}");
        Self {
            popup: None,
            bins: bin,
            elfs: elf,
            profiles_selected: false,
        }
    }
    pub fn reload(&mut self) -> color_eyre::Result<()> {
        self.reset();

        let meow = fs::read_to_string("../../esp_profiles.toml")?;
        let SerializedEspFiles { bin, elf } = toml::from_str(&meow)?;
        debug!("{bin:#?}");

        self.bins = bin;
        self.elfs = elf;

        Ok(())
    }
    pub fn consume_event(&mut self, event: EspEvent) {
        match event {
            EspEvent::DeviceInfo(info) => {
                debug!("{info:#?}");
                let DeviceInfo {
                    chip,
                    revision,
                    crystal_frequency,
                    flash_size,
                    features,
                    mac_address,
                } = info;

                let rows: Vec<Row> = vec![
                    Row::new([
                        line!["Chip:"].right_aligned(),
                        line![chip.to_string().to_uppercase()].centered(),
                    ]),
                    Row::new([
                        line!["Revision:"].right_aligned(),
                        line![format!("{revision:?}")].centered(),
                    ]),
                    Row::new([
                        line!["Crystal Osc. Frequency:"].right_aligned(),
                        line![format!("{crystal_frequency}")].centered(),
                    ]),
                    Row::new([
                        line!["Flash Size:"].right_aligned(),
                        line![format!("{flash_size}")].centered(),
                    ]),
                    Row::new([
                        line!["Features:"].right_aligned(),
                        line![format!("{features:?}")].centered(),
                    ]),
                    Row::new([
                        line!["MAC Address:"].right_aligned(),
                        line![format!("{mac_address}")].centered(),
                    ]),
                ];

                let table = Table::new(
                    rows,
                    [Constraint::Percentage(60), Constraint::Percentage(40)],
                )
                .column_highlight_style(Style::new())
                .row_highlight_style(Style::new());

                self.popup = Some(EspPopup::DeviceInfo(table));
            }
            EspEvent::FlashProgress(progress) => match progress {
                FlashProgress::Init { chip, addr, size } => {
                    self.popup = Some(EspPopup::Flashing(Flashing {
                        chip,
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
                }
            },
            EspEvent::Connecting => self.popup = Some(EspPopup::Connecting),
            EspEvent::Connected { chip } => self.popup = Some(EspPopup::Connected { chip }),
            EspEvent::EraseStart { chip } => self.popup = Some(EspPopup::Erasing { chip }),
            EspEvent::PortReturned => self.popup = None,
            _ => (),
        }
    }
    pub fn reset(&mut self) {
        _ = self.popup.take();
        self.profiles_selected = false;
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
            EspPopup::Erasing { .. } | EspPopup::Connected { .. } | EspPopup::Connecting => {
                Cow::from("")
            }
            EspPopup::DeviceInfo { .. } => Cow::from(" Retrieved ESP Info "),
            EspPopup::Flashing(Flashing { chip, .. }) => Cow::from(format!(" Flashing {chip}... ")),
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
                chip,
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

                let ratio = if *current_file_len == 0 {
                    0.0
                } else {
                    *current_progress as f64 / *current_file_len as f64
                };

                let label = format!("{:.2}%", ratio * 100.0);
                let progressbar = Gauge::default()
                    .gauge_style(Style::from(Color::Green))
                    .label(label)
                    .ratio(ratio);

                frame.render_widget(progressbar, progress_area);
            }
            EspPopup::Connecting => {
                frame.render_widget(
                    line!["Connecting to Espressif device..."].centered(),
                    body1_area,
                );
                frame.render_widget(
                    line!["Try holding down BOOT/IO0"].centered().dark_gray(),
                    chunks_text,
                );
                frame.render_widget(
                    line!["during connection if unreliable."]
                        .centered()
                        .dark_gray(),
                    which_bytes,
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
    pub fn profiles_table(&self, table_state: &mut TableState) -> Table {
        table_state.select_first_column();
        let selected_row_style = Style::new().reversed();
        let first_column_style = Style::new().reset();

        let rows: Vec<_> = self
            .profiles()
            .map(|(name, is_elf, is_elf_ram)| {
                let flavor = if is_elf_ram {
                    "Load ELF!"
                } else if is_elf {
                    "Flash ELF!"
                } else {
                    "Flash BIN!"
                };
                Row::new([
                    Text::raw(format!("{} ", name)).right_aligned(),
                    Text::raw(flavor).centered().italic(),
                ])
            })
            .collect();

        let option_table = Table::new(
            rows,
            [Constraint::Percentage(60), Constraint::Percentage(40)],
        )
        .column_highlight_style(first_column_style)
        .row_highlight_style(selected_row_style);

        option_table
    }
    pub fn profiles(&self) -> impl DoubleEndedIterator<Item = (&str, bool, bool)> {
        let elf_iter = self.elfs.iter().map(|e| (e.name.as_str(), true, e.ram));
        let bin_iter = self.bins.iter().map(|b| (b.name.as_str(), false, false));

        elf_iter.chain(bin_iter)
    }
    pub fn profile_from_name(&self, query: &str) -> Option<EspProfile> {
        // Search elfs first
        if let Some(elf) = self.elfs.iter().find(|e| e.name == query) {
            return Some(EspProfile::Elf(elf.clone()));
        }
        // Then bins
        if let Some(bin) = self.bins.iter().find(|b| b.name == query) {
            return Some(EspProfile::Bins(bin.clone()));
        }
        None
    }
    pub fn profile_from_index(&self, index: usize) -> Option<EspProfile> {
        // Elfs first, then bins, matching the order in profiles()
        let elf_len = self.elfs.len();
        if index < elf_len {
            self.elfs.get(index).cloned().map(EspProfile::Elf)
        } else {
            let bin_index = index - elf_len;
            self.bins.get(bin_index).cloned().map(EspProfile::Bins)
        }
    }
    pub fn is_empty(&self) -> bool {
        self.elfs.is_empty() && self.bins.is_empty()
    }
}

impl LastIndex for EspFlashState {
    fn last_index_checked(&self) -> Option<usize> {
        if self.is_empty() {
            None
        } else {
            Some((self.elfs.len() + self.bins.len()) - 1)
        }
    }
    fn last_index_eq(&self, index: usize) -> bool {
        if self.is_empty() {
            false
        } else if index == (self.elfs.len() + self.bins.len()) - 1 {
            true
        } else {
            false
        }
    }
    fn last_index_eq_or_greater(&self, index: usize) -> bool {
        if self.is_empty() {
            false
        } else if index >= (self.elfs.len() + self.bins.len()) - 1 {
            true
        } else {
            false
        }
    }
}

pub const ESPFLASH_BUTTON_COUNT: usize = 4;

pub fn espflash_buttons(table_state: &mut TableState) -> Table<'static> {
    table_state.select_first_column();
    let selected_row_style = Style::new().reversed();
    let first_column_style = Style::new().reset();

    let rows: Vec<Row> = vec![
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

    let option_table = Table::new(
        rows,
        [Constraint::Percentage(60), Constraint::Percentage(40)],
    )
    .column_highlight_style(first_column_style)
    .row_highlight_style(selected_row_style);

    option_table
}
