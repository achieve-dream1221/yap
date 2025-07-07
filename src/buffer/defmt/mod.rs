use camino::Utf8PathBuf;
use defmt_decoder::{DecodeError, Locations, Table};
use fs_err as fs;
use tracing::warn;

// #[ouroboros::self_referencing]
pub struct DefmtDecoder {
    elf_data: Vec<u8>,
    pub elf_md5: String,
    pub elf_path: Utf8PathBuf,
    pub table: Table,
    // #[borrows(table)]
    // #[covariant]
    // pub decoder: Box<dyn StreamDecoder + 'this>,
    pub locations: Option<Locations>,
}

#[cfg(feature = "defmt_watch")]
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
    #[error("error reading elf: {0}")]
    File(#[from] std::io::Error),
    #[error("path to elf is not utf-8: {0:?}")]
    NonUtf8Path(std::path::PathBuf),
}

// Much taken from defmt-print
// https://github.com/knurling-rs/defmt/blob/d52b9908c175497d46fc527f4f8dfd6278744f09/print/src/main.rs#L183

// include_bytes!(
//     "/home/tony/git/yap/defmt-meow-no-wire-debug"
// )

impl DefmtDecoder {
    fn from_elf_bytes(bytes: &[u8]) -> Result<(Table, Option<Locations>), DefmtTableError> {
        let table = Table::parse(bytes)
            .map_err(|e| DefmtTableError::ParseFail(e.to_string()))?
            .ok_or(DefmtTableError::DataMissing)?;

        let locs = table
            .get_locations(bytes)
            .map_err(|_| DefmtTableError::Locations)?;

        // TODO notify in UI
        let locations = if !table.is_empty() && locs.is_empty() {
            warn!(
                "Insufficient DWARF info; compile your program with `debug = 2` to enable location info."
            );
            None
        } else if table.indices().all(|idx| locs.contains_key(&(idx as u64))) {
            Some(locs)
        } else {
            warn!("(BUG) location info is incomplete; it will be omitted from the output");
            None
        };

        Ok((table, locations))
    }
    pub fn from_elf_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self, DefmtLoadError> {
        let path = path.as_ref().to_owned();
        let bytes = fs::read(&path)?;

        let elf_path = Utf8PathBuf::from_path_buf(path).map_err(DefmtLoadError::NonUtf8Path)?;

        let elf_data = bytes.to_owned();

        let (table, locations) = Self::from_elf_bytes(&bytes)?;

        let decoder = DefmtDecoder {
            elf_md5: format!("{:X}", md5::compute(&elf_data)),
            elf_path,
            elf_data,
            locations,
            table,
        };

        Ok(decoder)
    }
}

// Shamelessly stolen from
// https://github.com/esp-rs/espflash/blob/2c56b23fdf046be5019f22e4621d215ae01cfdc1/espflash/src/cli/monitor/parser/esp_defmt.rs
//
// I don't intend on keeping this exactly like they have it forever, it's just a good starting-off point.

// #[derive(Debug)]
// pub struct FrameDelimiter {
//     buffer: Vec<u8>,
//     in_frame: bool,
// }

// Framing info added by esp-println

// impl FrameDelimiter {
//     pub fn new() -> Self {
//         Self {
//             buffer: Vec::new(),
//             in_frame: false,
//         }
//     }

//     pub fn search(haystack: &[u8], look_for_end: bool) -> Option<(&[u8], usize)> {
//         let needle = if look_for_end { FRAME_END } else { FRAME_START };
//         let start = if look_for_end {
//             // skip leading zeros
//             haystack.iter().position(|&b| b != 0)?
//         } else {
//             0
//         };

//         let end = haystack[start..]
//             .windows(needle.len())
//             .position(|window| window == needle)?;

//         let end_extra = if look_for_end { needle.len() } else { 0 };

//         Some((
//             &haystack[start..][..end + end_extra],
//             start + end + needle.len(),
//         ))
//     }

//     /// Feeds data into the parser, extracting and processing framed or raw
//     /// data.
//     pub fn feed(&mut self, buffer: &[u8], mut process: impl FnMut(DefmtDelimitedSlice<'_>)) {
//         self.buffer.extend_from_slice(buffer);
//         debug!("feeding {} bytes", buffer.len());
//         debug!("{buffer:?}");
//         while let Some((frame, consumed)) = Self::search(&self.buffer, self.in_frame) {
//             debug!(
//                 "in_frame: {} | frame len: {} | consumed: {}",
//                 self.in_frame,
//                 frame.len(),
//                 consumed
//             );
//             if self.in_frame {
//                 process(DefmtDelimitedSlice::DefmtRzcobs {
//                     raw: &self.buffer[..consumed],
//                     inner: frame,
//                 });
//             } else if !frame.is_empty() {
//                 process(DefmtDelimitedSlice::Raw(frame));
//             }
//             self.in_frame = !self.in_frame;

//             self.buffer.drain(..consumed);
//         }

//         if !self.in_frame {
//             // If we have a 0xFF byte at the end, we should assume it's the start of a new
//             // frame.
//             let consume = if self.buffer.ends_with(&[0xFF]) {
//                 &self.buffer[..self.buffer.len() - 1]
//             } else {
//                 self.buffer.as_slice()
//             };

//             if !consume.is_empty() {
//                 process(DefmtDelimitedSlice::Raw(consume));
//                 self.buffer.drain(..consume.len());
//             }
//         }
//     }
// }

// pub struct ProcessedFrame<'a> {
//     level: Option<Level>,
//     location: Option<&'a Location>,
// }

/// Decode a full message.
///
/// `data` must be a full rzCOBS encoded message. Decoding partial
/// messages is not possible. `data` must NOT include any `0x00` separator byte.
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
