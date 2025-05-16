use compact_str::{CompactString, ToCompactString};
use espflash::{
    connection::reset::ResetStrategy,
    flasher::{DeviceInfo, ProgressCallbacks},
};
use std::{sync::mpsc::Sender, time::Duration};
use tracing::debug;

use super::{
    SerialEvent,
    handle::{SerialCommand, SerialHandle},
};
use crate::{
    app::Event,
    errors::{YapError, YapResult},
    traits::LastIndex,
    tui::esp::EspBins,
};

#[derive(Debug)]
pub enum EspCommand {
    EraseFlash,
    WriteBins(EspBins),
    Restart { bootloader: bool },
    DeviceInfo,
}

impl SerialHandle {
    pub fn esp_restart(&self, bootloader: bool) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Esp(EspCommand::Restart { bootloader }))
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_device_info(&self) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Esp(EspCommand::DeviceInfo))
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_write_bins(&self, bins: EspBins) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Esp(EspCommand::WriteBins(bins)))
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_erase_flash(&self) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Esp(EspCommand::EraseFlash))
            .map_err(|_| YapError::NoSerialWorker)
    }
}

#[derive(Debug, Clone)]
pub enum EspFlashEvent {
    Connecting,
    Connected { chip: CompactString },
    BootloaderSuccess { chip: CompactString },
    EraseStart { chip: CompactString },
    EraseSuccess { chip: CompactString },
    HardResetAttempt,
    DeviceInfo(DeviceInfo),
    FlashProgress(FlashProgress),
    Error(String),
    PortReturned,
}

#[derive(Debug, Clone)]
pub enum FlashProgress {
    Init {
        // name: CompactString,
        addr: u32,
        size: usize,
    },
    Progress(usize),
    SegmentFinished,
}

impl From<EspFlashEvent> for SerialEvent {
    fn from(value: EspFlashEvent) -> Self {
        Self::EspFlash(value)
    }
}

impl From<EspFlashEvent> for Event {
    fn from(value: EspFlashEvent) -> Self {
        Self::Serial(SerialEvent::EspFlash(value))
    }
}

impl From<FlashProgress> for Event {
    fn from(value: FlashProgress) -> Self {
        Self::Serial(SerialEvent::EspFlash(EspFlashEvent::FlashProgress(value)))
    }
}

pub struct ProgressPropagator {
    tx: Sender<Event>,
    // filenames: Vec<&'a str>,
    // current_index: i16,
}
impl ProgressPropagator {
    pub fn new(tx: Sender<Event>) -> Self {
        Self {
            tx,
            // filenames: Vec::from_iter(filenames.iter().map(AsRef::as_ref)),
            // current_index: -1,
        }
    }
}
impl ProgressCallbacks for ProgressPropagator {
    fn init(&mut self, addr: u32, total: usize) {
        // assert!(
        //     self.filenames.len() <= u8::MAX as usize,
        //     "Not supporting more than 255 files per profile."
        // );
        // self.current_index += 1;

        _ = self.tx.send(
            FlashProgress::Init {
                addr,
                size: total,
                // name: self.filenames[self.current_index as usize].to_compact_string(),
            }
            .into(),
        );
    }
    fn update(&mut self, current: usize) {
        _ = self.tx.send(FlashProgress::Progress(current).into());
    }
    fn finish(&mut self) {
        _ = self.tx.send(FlashProgress::SegmentFinished.into());
    }
}

pub struct TestReset {
    delay: u64,
}
impl TestReset {
    pub fn new() -> Self {
        Self { delay: 50 }
    }
}
impl ResetStrategy for TestReset {
    fn reset(
        &self,
        serial_port: &mut espflash::connection::Port,
    ) -> Result<(), espflash::error::Error> {
        debug!(
            "Using Classic reset strategy with delay of {}ms",
            self.delay
        );
        self.set_dtr(serial_port, false)?;
        self.set_rts(serial_port, false)?;

        self.set_dtr(serial_port, true)?;
        self.set_rts(serial_port, true)?;

        self.set_dtr(serial_port, false)?; // IO0 = HIGH
        self.set_rts(serial_port, true)?; // EN = LOW, chip in reset

        std::thread::sleep(Duration::from_millis(100));

        self.set_dtr(serial_port, true)?; // IO0 = LOW
        self.set_rts(serial_port, false)?; // EN = HIGH, chip out of reset

        std::thread::sleep(Duration::from_millis(self.delay));

        self.set_dtr(serial_port, false)?; // IO0 = HIGH, done
        self.set_rts(serial_port, false)?;

        Ok(())
    }
}
