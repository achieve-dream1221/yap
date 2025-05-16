use std::path::PathBuf;

use compact_str::CompactString;
use espflash::{flasher::DeviceInfo, targets::Chip};
use fs_err as fs;
use ratatui::{
    prelude::*,
    widgets::{Row, Table, TableState},
};
use tracing::debug;

use crate::serial::esp::EspFlashEvent;

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct EspBins {
    pub name: CompactString,
    pub macro_name: Option<CompactString>,
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
                let mut macro_name = None;
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
                        "macro_name" => {
                            let value: CompactString = map.next_value()?;
                            macro_name = Some(value);
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
                    macro_name,
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

#[derive(Debug)]
pub struct EspFlashState {
    info: Option<DeviceInfo>,
    pub bins: Vec<EspBins>,
}

impl EspFlashState {
    pub fn new() -> Self {
        let meow = fs::read_to_string("../../esp-profiles.toml").unwrap();
        let SerializedEspFiles { bin } = toml::from_str(&meow).unwrap();
        debug!("{bin:#?}");
        Self {
            info: None,
            bins: bin,
        }
    }
    pub fn consume_event(&mut self, event: EspFlashEvent) {
        match event {
            EspFlashEvent::DeviceInfo(info) => {
                debug!("{info:#?}");
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
