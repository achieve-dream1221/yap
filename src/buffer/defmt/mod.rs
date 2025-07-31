use camino::Utf8PathBuf;
use defmt_decoder::{DecodeError, Locations, Table};
use fs_err as fs;
use tracing::warn;

// Huge inspiration from defmt-print
// https://github.com/knurling-rs/defmt/blob/d52b9908c175497d46fc527f4f8dfd6278744f09/print/src/main.rs#L183
// and espflash
// https://github.com/esp-rs/espflash/blob/b993a42fe48f4e679d687d927ba15d73ef495b1f/espflash/src/cli/monitor/parser/esp_defmt.rs

pub struct DefmtDecoder {
    // might need it at some point, who knows.
    // elf_data: Vec<u8>,
    pub elf_md5: String,
    pub elf_path: Utf8PathBuf,
    pub table: Table,
    pub locations: Option<Locations>,
}

#[cfg(feature = "defmt-watch")]
pub mod elf_watcher;

pub mod frame_delimiting;

#[derive(Debug, thiserror::Error)]
pub enum DefmtPacketError {
    #[error("no defmt table loaded")]
    NoDecoder,
    #[error("rzcobs decompress failed")]
    RzcobsDecompress,
    #[error("packet decode failed")]
    DefmtDecode,
}

#[derive(Debug, thiserror::Error)]
pub enum DefmtTableError {
    #[error("defmt data missing")]
    DataMissing,
    #[error("locations get failed")]
    Locations,
    #[error("elf parse failed")]
    ParseFail(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DefmtLoadError {
    #[error(transparent)]
    ElfParse(#[from] DefmtTableError),
    #[error("error reading elf")]
    File(#[from] std::io::Error),
    #[error("path to elf is not utf-8")]
    NonUtf8Path(std::path::PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum LocationsError {
    #[error("missing, compile with `debug = 2` to enable")]
    InsufficientDwarfInfo,
    #[error("bugged elf, ignoring location data")]
    IncompleteLocations,
}

impl DefmtDecoder {
    fn from_elf_bytes(
        bytes: &[u8],
    ) -> Result<(Table, Result<Locations, LocationsError>), DefmtTableError> {
        let table = Table::parse(bytes)
            .map_err(|e| DefmtTableError::ParseFail(e.to_string()))?
            .ok_or(DefmtTableError::DataMissing)?;

        let locs = table
            .get_locations(bytes)
            .map_err(|_| DefmtTableError::Locations)?;

        let locations = if !table.is_empty() && locs.is_empty() {
            warn!(
                "Insufficient DWARF info; compile your program with `debug = 2` to enable location info."
            );
            Err(LocationsError::InsufficientDwarfInfo)
        } else if table.indices().all(|idx| locs.contains_key(&(idx as u64))) {
            Ok(locs)
        } else {
            warn!("(BUG) location info is incomplete; it will be omitted from the output");
            Err(LocationsError::IncompleteLocations)
        };

        Ok((table, locations))
    }
    pub fn from_elf_path<P: AsRef<std::path::Path>>(
        path: P,
    ) -> Result<(Self, Option<LocationsError>), DefmtLoadError> {
        let path = path.as_ref().to_owned();
        let bytes = fs::read(&path)?;

        let elf_path = Utf8PathBuf::from_path_buf(path).map_err(DefmtLoadError::NonUtf8Path)?;

        let elf_data = bytes.to_owned();

        let (table, locations_res) = Self::from_elf_bytes(&bytes)?;

        let (locations, locations_err) = match locations_res {
            Ok(locs) => (Some(locs), None),
            Err(err) => (None, Some(err)),
        };

        let decoder = DefmtDecoder {
            elf_md5: format!("{:X}", md5::compute(&elf_data)),
            elf_path,
            locations,
            table,
            // elf_data,
        };

        Ok((decoder, locations_err))
    }
}

// Variant of
// https://github.com/Dirbaio/rzcobs/blob/d74339bf1a9e93ea5a9417deaececccd145139c0/src/lib.rs#L202
/// Decode a full message.
///
/// `data` must be a full rzCOBS encoded message. Decoding partial
/// messages is not possible.
///
/// `data` must NOT include any `0x00` separator byte.
pub fn rzcobs_decode(data: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut res = vec![];
    let mut data = data.iter().rev().cloned();
    while let Some(x) = data.next() {
        match x {
            0 => return Err(DecodeError::Malformed),
            0x01..=0x7f => {
                for i in 0..7 {
                    if x & (1 << (6 - i)) == 0 {
                        res.push(data.next().ok_or(DecodeError::Malformed)?);
                    } else {
                        res.push(0);
                    }
                }
            }
            0x80..=0xfe => {
                let n = (x & 0x7f) + 7;
                res.push(0);
                for _ in 0..n {
                    res.push(data.next().ok_or(DecodeError::Malformed)?);
                }
            }
            0xff => {
                for _ in 0..134 {
                    res.push(data.next().ok_or(DecodeError::Malformed)?);
                }
            }
        }
    }

    res.reverse();
    Ok(res)
}
