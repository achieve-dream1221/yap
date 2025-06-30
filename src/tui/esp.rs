use std::{borrow::Cow, f32::consts::PI, path::PathBuf};

use camino::Utf8PathBuf;
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

const ESP_PROFILES_PATH: &str = "yap_espflash_profiles.toml";

use serde::Deserialize;

use super::centered_rect_size;

// TODO move file stuff out of TUI module

#[derive(Debug)]
pub enum EspProfile {
    Bins(EspBins),
    Elf(EspElf),
}

#[cfg(feature = "defmt")]
impl EspProfile {
    pub fn defmt_elf_path(&self) -> Option<Utf8PathBuf> {
        match self {
            EspProfile::Bins(EspBins {
                defmt_elf_path: Some(path),
                ..
            }) => Some(path.to_owned()),

            EspProfile::Elf(EspElf {
                path, defmt: true, ..
            }) => Some(path.to_owned()),

            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EspBins {
    pub name: CompactString,
    pub bins: Vec<(u32, Utf8PathBuf)>,
    pub upload_baud: Option<u32>,
    pub expected_chip: Option<Chip>,
    // pub partition_table: Option<PathBuf>,
    pub no_skip: bool,
    pub no_verify: bool,
    #[cfg(feature = "defmt")]
    pub defmt_elf_path: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone)]
pub struct EspElf {
    pub name: CompactString,
    pub path: Utf8PathBuf,
    pub upload_baud: Option<u32>,
    pub expected_chip: Option<Chip>,
    pub partition_table: Option<Utf8PathBuf>,
    pub bootloader: Option<Utf8PathBuf>,
    pub no_skip: bool,
    pub no_verify: bool,
    pub ram: bool,
    #[cfg(feature = "defmt")]
    pub defmt: bool,
}

impl<'de> serde::Deserialize<'de> for EspElf {
    fn deserialize<D>(deserializer: D) -> Result<EspElf, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, MapAccess, Visitor};
        use std::fmt;

        struct EspElfVisitor;

        impl<'de> Visitor<'de> for EspElfVisitor {
            type Value = EspElf;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map representing EspElf with validation")
            }

            fn visit_map<A>(self, mut map: A) -> Result<EspElf, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut name = None;
                let mut path = None;
                let mut upload_baud = None;
                let mut expected_chip = None;
                let mut partition_table = None;
                let mut bootloader = None;
                let mut no_skip = false;
                let mut no_verify = false;
                let mut ram = false;
                #[cfg(feature = "defmt")]
                let mut defmt = false;
                let mut any_chip = false;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => {
                            let value: CompactString = map.next_value()?;
                            name = Some(value);
                        }
                        "path" => {
                            let value: Utf8PathBuf = map.next_value()?;
                            path = Some(value);
                        }
                        "upload_baud" => {
                            upload_baud = Some(map.next_value()?);
                        }
                        "chip" => {
                            let value: String = map.next_value()?;
                            if value == "any" {
                                any_chip = true;
                            } else {
                                expected_chip =
                                    Some(value.parse::<Chip>().map_err(|_| {
                                        A::Error::custom("invalid chip type given")
                                    })?);
                            }
                        }
                        "partition_table" => {
                            partition_table = Some(map.next_value()?);
                        }
                        "bootloader" => {
                            bootloader = Some(map.next_value()?);
                        }
                        "no_skip" => {
                            no_skip = map.next_value()?;
                        }
                        "no_verify" => {
                            no_verify = map.next_value()?;
                        }
                        "ram" => {
                            ram = map.next_value()?;
                        }
                        #[cfg(feature = "defmt")]
                        "defmt" => {
                            defmt = map.next_value()?;
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let name = name.ok_or_else(|| A::Error::missing_field("name"))?;
                let path = path.ok_or_else(|| A::Error::missing_field("path"))?;

                if !any_chip && expected_chip.is_none() {
                    return Err(A::Error::missing_field("chip"));
                }

                Ok(EspElf {
                    name,
                    path,
                    upload_baud,
                    expected_chip,
                    partition_table,
                    bootloader,
                    no_skip,
                    no_verify,
                    ram,
                    #[cfg(feature = "defmt")]
                    defmt,
                })
            }
        }

        deserializer.deserialize_map(EspElfVisitor)
    }
}

// fn deserialize_chip<'de, D>(deserializer: D) -> Result<Option<Chip>, D::Error>
// where
//     D: serde::Deserializer<'de>,
// {
//     use serde::de::Error;
//     use std::str::FromStr;
//     let opt = Option::<String>::deserialize(deserializer)?;
//     match opt {
//         Some(s) => Chip::from_str(&s)
//             .map(Some)
//             .map_err(|_| D::Error::custom("invalid chip type given")),
//         None => Ok(None),
//     }
// }

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
                let mut any_chip = false;
                #[cfg(feature = "defmt")]
                let mut defmt_elf_path = None;

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
                            if value == "any" {
                                any_chip = true;
                            } else {
                                expected_chip =
                                    Some(value.parse::<Chip>().map_err(|_| {
                                        A::Error::custom("invalid chip type given")
                                    })?);
                            }
                        }
                        "no_verify" => {
                            no_verify = map.next_value()?;
                        }
                        "no_skip" => {
                            no_skip = map.next_value()?;
                        }
                        #[cfg(feature = "defmt")]
                        "defmt_elf_path" => {
                            defmt_elf_path = map.next_value()?;
                        }
                        other if other.starts_with("0x") => {
                            let offset_num =
                                u32::from_str_radix(other.trim_start_matches("0x"), 16).map_err(
                                    |_| {
                                        A::Error::custom(format!("invalid bin offset key: {other}"))
                                    },
                                )?;
                            let path_val: String = map.next_value()?;
                            bins.push((offset_num, Utf8PathBuf::from(path_val)));
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let name = name.ok_or_else(|| A::Error::missing_field("name"))?;

                if !any_chip && expected_chip.is_none() {
                    return Err(A::Error::missing_field("chip"));
                }

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
                    #[cfg(feature = "defmt")]
                    defmt_elf_path,
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
}

impl EspFlashState {
    pub fn new() -> Self {
        let meow = fs::read_to_string(ESP_PROFILES_PATH).unwrap();
        let SerializedEspFiles { bin, elf } = toml::from_str(&meow).unwrap();
        debug!("{bin:#?}");
        Self {
            popup: None,
            bins: bin,
            elfs: elf,
        }
    }
    pub fn reload(&mut self) -> color_eyre::Result<()> {
        self.reset_popup();

        let meow = fs::read_to_string(ESP_PROFILES_PATH)?;
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
    pub fn reset_popup(&mut self) {
        _ = self.popup.take();
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
    pub fn profiles_table(&self) -> Table {
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
    pub fn len(&self) -> usize {
        self.elfs.len() + self.bins.len()
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
}

pub const ESPFLASH_BUTTON_COUNT: usize = 4;

pub fn espflash_buttons() -> Table<'static> {
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
