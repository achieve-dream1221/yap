use compact_str::CompactString;
use crossbeam::channel::Sender;
use espflash::{flasher::DeviceInfo, target::ProgressCallbacks};

use super::{
    SerialEvent,
    handle::{PortCommand, SerialHandle, SerialWorkerCommand},
};
use crate::{app::Event, serial::handle::SerialWorkerMissing, tui::esp::EspProfile};

#[derive(Debug)]
pub enum EspCommand {
    EraseFlash,
    FlashProfile(EspProfile),
    Restart(EspRestartType),
    DeviceInfo,
}

#[derive(Debug)]
pub enum EspRestartType {
    /// Attempt to restart the ESP into the ROM booloader.
    ///
    /// If `active` is `true`, will give several attempts until success is confirmed or enough failures occur.
    ///
    /// Otherwise, will try once to do the DTR-RTS dance to restart the ESP into the bootloader,
    /// and will not check for success.
    Bootloader { active: bool },
    /// Will try once to do the DTR-RTS dance to restart the ESP into the flashed firmware,
    /// not checking for success.
    UserCode,
}

impl From<EspCommand> for SerialWorkerCommand {
    fn from(value: EspCommand) -> Self {
        SerialWorkerCommand::PortCommand(PortCommand::Esp(value))
    }
}

type HandleResult<T> = Result<T, SerialWorkerMissing>;

impl SerialHandle {
    /// Ask the serial worker to attempt to restart the connected ESP
    pub fn esp_restart(&self, restart_type: EspRestartType) -> HandleResult<()> {
        self.command_tx
            .send(EspCommand::Restart(restart_type).into())?;
        Ok(())
    }

    /// Ask the serial worker to attempt to query ESP device info
    pub fn esp_device_info(&self) -> HandleResult<()> {
        self.command_tx.send(EspCommand::DeviceInfo.into())?;
        Ok(())
    }

    /// Ask the serial worker to attempt to flash a given set of files using espflash
    pub fn esp_flash_profile(&self, profile: EspProfile) -> HandleResult<()> {
        self.command_tx
            .send(EspCommand::FlashProfile(profile).into())?;
        Ok(())
    }

    /// Ask the serial worker to connect to the device as an ESP and request a flash erase.
    pub fn esp_erase_flash(&self) -> HandleResult<()> {
        self.command_tx.send(EspCommand::EraseFlash.into())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum EspEvent {
    /// Serial worker is attempting to connect to the port as an ESP.
    Connecting,
    /// Serial worker was able to connect to ESP, silicon variant was read and sent.
    Connected {
        chip: CompactString,
    },
    /// Serial worker successfully rebooted ESP into bootloader.
    BootloaderSuccess {
        chip: CompactString,
    },
    /// Serial worker blindly attempted to reboot ESP into bootloader.
    BootloaderAttempt,
    /// Serial worker blindly attempted to reboot ESP into user code.
    HardResetAttempt,
    /// ESP has began erasing all flash contents
    EraseStart {
        chip: CompactString,
    },
    /// ESP has finished erasing all flash contents
    EraseSuccess {
        chip: CompactString,
    },
    /// Serial worker successfully queried ESP device info
    DeviceInfo(DeviceInfo),
    FlashProgress(FlashProgress),
    Error(String),
    /// Port ownership has been returned from espflash back to the serial worker.
    PortReturned,
}

#[derive(Debug, Clone)]
pub enum FlashProgress {
    /// Info of segment-to-flash
    SegmentInit {
        chip: CompactString,
        addr: u32,
        size: usize,
        file_name: Option<String>,
    },
    /// Current progress of segment: progress <= SegmentInit.size
    Progress(usize),
    /// Segment has finished flashing, awaiting MD5 hash from ESP to compare results.
    Verifying,
    /// Segment has finished entirely.
    SegmentFinished {
        /// Indicates if espflash chose to skip flashing
        /// due to the target's flash segment's contents hash
        /// already matches the MD5 hash of the segment-to-flash.
        skipped: bool,
    },
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

/// Progress callback object passed to espflash
/// When applicable, will try to report the name of the file being flashed.
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
            FlashProgress::SegmentInit {
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
    fn verifying(&mut self) {
        _ = self.tx.send(FlashProgress::Verifying.into());
    }
    fn finish(&mut self, skipped: bool) {
        _ = self
            .tx
            .send(FlashProgress::SegmentFinished { skipped }.into());
        self.current_index += 1;
    }
}

// pub struct TestReset {
//     delay: u64,
// }
// impl TestReset {
//     pub fn new() -> Self {
//         Self { delay: 50 }
//     }
// }
// impl ResetStrategy for TestReset {
//     fn reset(
//         &self,
//         serial_port: &mut espflash::connection::Port,
//     ) -> Result<(), espflash::error::Error> {
//         debug!(
//             "Using Classic reset strategy with delay of {}ms",
//             self.delay
//         );
//         self.set_dtr(serial_port, false)?;
//         self.set_rts(serial_port, false)?;

//         self.set_dtr(serial_port, true)?;
//         self.set_rts(serial_port, true)?;

//         self.set_dtr(serial_port, false)?; // IO0 = HIGH
//         self.set_rts(serial_port, true)?; // EN = LOW, chip in reset

//         std::thread::sleep(Duration::from_millis(100));

//         self.set_dtr(serial_port, true)?; // IO0 = LOW
//         self.set_rts(serial_port, false)?; // EN = HIGH, chip out of reset

//         std::thread::sleep(Duration::from_millis(self.delay));

//         self.set_dtr(serial_port, false)?; // IO0 = HIGH, done
//         self.set_rts(serial_port, false)?;

//         Ok(())
//     }
// }
