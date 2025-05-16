use espflash::connection::reset::ResetStrategy;
use std::time::Duration;
use tracing::debug;

use super::SerialEvent;
use crate::app::Event;

#[derive(Debug, Clone)]
pub enum EspFlashEvent {
    PortBorrowed,
    BootloaderSuccess { chip: String },
    HardResetAttempt,
    Error(String),
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
