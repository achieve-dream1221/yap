use compact_str::{CompactString, ToCompactString};
use espflash::{
    connection::reset::ResetStrategy,
    flasher::{DeviceInfo, ProgressCallbacks},
};
use std::{sync::mpsc::Sender, time::Duration};
use tracing::debug;

use super::{
    SerialEvent,
    handle::{PortCommand, SerialHandle, SerialWorkerCommand},
};
use crate::{
    app::Event,
    errors::{YapError, YapResult},
    tui::esp::{EspBins, EspProfile},
};

#[derive(Debug)]
pub enum EspCommand {
    EraseFlash,
    FlashProfile(EspProfile),
    Restart(EspRestartType),
    DeviceInfo,
}

#[derive(Debug)]
pub enum EspRestartType {
    Bootloader { active: bool },
    UserCode,
}

impl From<EspCommand> for SerialWorkerCommand {
    fn from(value: EspCommand) -> Self {
        SerialWorkerCommand::PortCommand(PortCommand::Esp(value))
    }
}

impl SerialHandle {
    pub fn esp_restart(&self, restart_type: EspRestartType) -> YapResult<()> {
        self.command_tx
            .send(EspCommand::Restart(restart_type).into())
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_device_info(&self) -> YapResult<()> {
        self.command_tx
            .send(EspCommand::DeviceInfo.into())
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_flash_profile(&self, profile: EspProfile) -> YapResult<()> {
        self.command_tx
            .send(EspCommand::FlashProfile(profile).into())
            .map_err(|_| YapError::NoSerialWorker)
    }

    pub fn esp_erase_flash(&self) -> YapResult<()> {
        self.command_tx
            .send(EspCommand::EraseFlash.into())
            .map_err(|_| YapError::NoSerialWorker)
    }
}

#[derive(Debug, Clone)]
pub enum EspEvent {
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
        chip: CompactString,
        addr: u32,
        size: usize,
    },
    Progress(usize),
    SegmentFinished,
    // TODO Verifying + Skipping popups
}

impl From<EspEvent> for SerialEvent {
    fn from(value: EspEvent) -> Self {
        Self::EspFlash(value)
    }
}

impl From<EspEvent> for Event {
    fn from(value: EspEvent) -> Self {
        Self::Serial(SerialEvent::EspFlash(value))
    }
}

impl From<FlashProgress> for Event {
    fn from(value: FlashProgress) -> Self {
        Self::Serial(SerialEvent::EspFlash(EspEvent::FlashProgress(value)))
    }
}

pub struct ProgressPropagator {
    chip: CompactString,
    tx: Sender<Event>,
    // filenames: Vec<&'a str>,
    // current_index: i16,
}
impl ProgressPropagator {
    pub fn new(tx: Sender<Event>, chip: CompactString) -> Self {
        Self {
            chip,
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
                chip: self.chip.clone(),
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
