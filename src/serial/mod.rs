use serialport::{SerialPort, SerialPortInfo, SerialPortType};

use crate::app::Event;

pub mod handle;
pub mod worker;

#[cfg(feature = "espflash")]
pub mod esp;
#[cfg(feature = "espflash")]
use esp::EspFlashEvent;

#[derive(Clone, Debug)]
pub enum ReconnectType {
    PerfectMatch,
    UsbStrict,
    UsbLoose,
    LastDitch,
}

#[derive(Debug, Clone)]
pub enum SerialEvent {
    Ports(Vec<SerialPortInfo>),
    Connected(Option<ReconnectType>),
    RxBuffer(Vec<u8>),
    #[cfg(feature = "espflash")]
    EspFlash(EspFlashEvent),
    Disconnected(Option<String>),
}

impl From<SerialEvent> for Event {
    fn from(value: SerialEvent) -> Self {
        Self::Serial(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Reconnections {
    Disabled,
    StrictChecks,
    LooseChecks,
}

impl std::fmt::Display for Reconnections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reconnection_str = match self {
            Reconnections::Disabled => "Disabled",
            Reconnections::StrictChecks => "Strict Checks",
            Reconnections::LooseChecks => "Loose Checks",
        };
        write!(f, "{}", reconnection_str)
    }
}

pub trait PrintablePortInfo {
    fn info_as_string(&self, baud_rate: Option<u32>) -> String;
}

impl PrintablePortInfo for SerialPortInfo {
    fn info_as_string(&self, baud_rate: Option<u32>) -> String {
        let extra_info = match &self.port_type {
            SerialPortType::UsbPort(usb) => {
                format!("VID: 0x{:04X}, PID: 0x{:04X}", usb.vid, usb.pid)
            }
            SerialPortType::Unknown => String::new(),
            SerialPortType::PciPort => "PCI".to_owned(),
            SerialPortType::BluetoothPort => "Bluetooth".to_owned(),
        };
        let port = &self.port_name;
        match (baud_rate, extra_info.is_empty()) {
            (Some(baud), false) => format!("{port} @ {baud} | {extra_info}"),
            (Some(baud), true) => format!("{port} @ {baud}"),
            (None, false) => format!("{port} | {extra_info}"),
            (None, true) => format!("{port}"),
        }

        // if extra_info.is_empty() {
        //     format!("{}", self.port_name)
        // } else {
        //     format!("{} | {extra_info}", self.port_name)
        // }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SerialSignals {
    // Host-controlled
    /// RTS (Request To Send)
    pub rts: bool,
    /// DTR (Data Terminal Ready)
    pub dtr: bool,
    // Slave-controlled, polled periodically
    /// CTS (Clear To Send)
    pub cts: bool,
    /// DSR (Data Set Ready)
    pub dsr: bool,
    /// RI (Ring Indicator)
    pub ri: bool,
    /// CD (Carrier Detect)
    pub cd: bool,
}

impl SerialSignals {
    // fn toggle_dtr(&mut self, serial_port: &mut dyn SerialPort) -> Result<bool, serialport::Error> {
    //     let dtr = !self.dtr;
    //     serial_port.write_data_terminal_ready(dtr)?;
    //     Ok(dtr)
    // }
    // fn toggle_rts(&mut self, serial_port: &mut dyn SerialPort) -> Result<bool, serialport::Error> {
    //     let rts = !self.rts;
    //     serial_port.write_request_to_send(rts)?;
    //     Ok(rts)
    // }
    // fn new_from_port(serial_port: &mut dyn SerialPort) -> Result<Self, serialport::Error> {
    //     let mut signals = Self::default();
    //     signals.update_with_port(serial_port)?;
    //     Ok(signals)
    // }
    fn update_with_port(
        &mut self,
        serial_port: &mut dyn SerialPort,
    ) -> Result<bool, serialport::Error> {
        let cts = serial_port.read_clear_to_send()?;
        let dsr = serial_port.read_data_set_ready()?;
        let ri = serial_port.read_ring_indicator()?;
        let cd = serial_port.read_carrier_detect()?;

        let changed = self.cts != cts || self.dsr != dsr || self.ri != ri || self.cd != cd;

        self.cts = cts;
        self.dsr = dsr;
        self.ri = ri;
        self.cd = cd;

        Ok(changed)
    }
}
