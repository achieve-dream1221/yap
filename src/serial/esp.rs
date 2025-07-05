use compact_str::{CompactString, ToCompactString};
use crossbeam::channel::{Receiver, Sender};
use espflash::{
    connection::reset::ResetStrategy,
    flasher::{DeviceInfo, ProgressCallbacks},
};
use std::time::Duration;
use tracing::debug;

use super::{
    SerialEvent,
    handle::{PortCommand, SerialHandle, SerialWorkerCommand},
};
use crate::{
    app::Event,
    errors::HandleResult,
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
    pub fn esp_restart(&self, restart_type: EspRestartType) -> HandleResult<()> {
        self.command_tx
            .send(EspCommand::Restart(restart_type).into())?;
        Ok(())
    }

    pub fn esp_device_info(&self) -> HandleResult<()> {
        self.command_tx.send(EspCommand::DeviceInfo.into())?;
        Ok(())
    }

    pub fn esp_flash_profile(&self, profile: EspProfile) -> HandleResult<()> {
        self.command_tx
            .send(EspCommand::FlashProfile(profile).into())?;
        Ok(())
    }

    pub fn esp_erase_flash(&self) -> HandleResult<()> {
        self.command_tx.send(EspCommand::EraseFlash.into())?;
        Ok(())
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
        file_name: Option<String>,
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

pub struct ProgressPropagator<'a> {
    chip: CompactString,
    tx: Sender<Event>,
    filenames: Vec<&'a str>,
    current_index: usize,
}
impl<'a> ProgressPropagator<'a> {
    pub fn new(tx: Sender<Event>, chip: CompactString, filenames: Vec<&'a str>) -> Self {
        Self {
            chip,
            tx,
            filenames,
            current_index: 0,
        }
    }
}
impl ProgressCallbacks for ProgressPropagator<'_> {
    fn init(&mut self, addr: u32, total: usize) {
        _ = self.tx.send(
            FlashProgress::Init {
                chip: self.chip.clone(),
                addr,
                size: total,
                file_name: self
                    .filenames
                    .get(self.current_index)
                    .map(ToString::to_string),
            }
            .into(),
        );
    }
    fn update(&mut self, current: usize) {
        _ = self.tx.send(FlashProgress::Progress(current).into());
    }
    fn finish(&mut self) {
        _ = self.tx.send(FlashProgress::SegmentFinished.into());
        self.current_index += 1;
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
