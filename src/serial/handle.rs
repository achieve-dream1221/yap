use std::{
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

use arc_swap::ArcSwap;
use bstr::ByteVec;
use serialport::SerialPortInfo;
use tracing::error;

use crate::{
    app::Event,
    errors::{YapError, YapResult},
    settings::PortSettings,
};

use super::worker::{PortStatus, SerialWorker};

#[derive(Debug)]
pub enum SerialCommand {
    RequestPortScan,
    Connect {
        port: SerialPortInfo,
        settings: PortSettings,
    },
    PortSettings(PortSettings),
    TxBuffer(Vec<u8>),
    #[cfg(feature = "espflash")]
    EspRestart {
        bootloader: bool,
    },
    WriteSignals {
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    ToggleSignals {
        dtr: bool,
        rts: bool,
    },
    // ReadSignals,
    RequestReconnect,
    Disconnect,
    Shutdown(Sender<()>),
}

#[derive(Clone)]
pub struct SerialHandle {
    command_tx: Sender<SerialCommand>,
    pub port_status: Arc<ArcSwap<PortStatus>>,
    pub port_settings: Arc<ArcSwap<PortSettings>>,
}

impl SerialHandle {
    pub fn new(event_tx: Sender<Event>, port_settings: PortSettings) -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = mpsc::channel();

        let port_status = Arc::new(ArcSwap::from_pointee(PortStatus::new_idle(&port_settings)));

        let port_settings = Arc::new(ArcSwap::from_pointee(port_settings));

        let mut worker = SerialWorker::new(
            command_rx,
            event_tx,
            port_status.clone(),
            port_settings.clone(),
        );

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("Serial worker encountered a fatal error!");
        });

        let handle = Self {
            command_tx,
            port_status,
            port_settings,
        };
        handle.request_port_scan().unwrap();
        (handle, worker)
    }
    pub fn connect(&self, port: &SerialPortInfo, settings: PortSettings) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Connect {
                port: port.to_owned(),
                settings,
            })
            .map_err(|_| YapError::NoSerialWorker)
    }
    pub fn disconnect(&self) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::Disconnect)
            .map_err(|_| YapError::NoSerialWorker)
    }
    pub fn update_settings(&self, settings: PortSettings) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::PortSettings(settings))
            .map_err(|_| YapError::NoSerialWorker)
    }
    /// Sends the supplied bytes through the connected Serial device.
    pub fn send_bytes(&self, mut input: Vec<u8>, line_ending: Option<&[u8]>) -> YapResult<()> {
        if let Some(ending) = line_ending {
            input.extend(ending.iter());
        }
        self.command_tx
            .send(SerialCommand::TxBuffer(input))
            .map_err(|_| YapError::NoSerialWorker)
    }
    pub fn send_str(&self, input: &str, line_ending: &[u8], unescape_bytes: bool) -> YapResult<()> {
        // debug!("Outputting to serial: {input}");
        let buffer = if unescape_bytes {
            Vec::unescape_bytes(input)
        } else {
            input.as_bytes().to_owned()
        };
        self.send_bytes(buffer, Some(line_ending))
    }
    // pub fn read_signals(&self) -> YapResult<()> {
    //     self.command_tx
    //         .send(SerialCommand::ReadSignals)
    //         .map_err(|_| YapError::NoSerialWorker)
    // }
    #[cfg(feature = "espflash")]
    pub fn esp_restart(&self, bootloader: bool) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::EspRestart { bootloader })
            .map_err(|_| YapError::NoSerialWorker)
    }
    pub fn write_signals(&self, dtr: Option<bool>, rts: Option<bool>) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::WriteSignals { dtr, rts })
            .map_err(|_| YapError::NoSerialWorker)
    }
    pub fn toggle_signals(&self, dtr: bool, rts: bool) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::ToggleSignals { dtr, rts })
            .map_err(|_| YapError::NoSerialWorker)
    }
    /// Non-blocking request for the serial worker to scan for ports and send a list of available ports
    pub fn request_port_scan(&self) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::RequestPortScan)
            .map_err(|_| YapError::NoSerialWorker)
    }
    /// Non-blocking request for the serial worker to attempt to reconnect to the "current" device
    pub fn request_reconnect(&self) -> YapResult<()> {
        self.command_tx
            .send(SerialCommand::RequestReconnect)
            .map_err(|_| YapError::NoSerialWorker)
    }
    // TODO maybe just shut down when we lose all Tx handles?
    // Would still want to detect a locked thread and handle it though
    /// Tells the worker thread to shutdown, blocking for up to three seconds before aborting.
    pub fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        if self
            .command_tx
            .send(SerialCommand::Shutdown(shutdown_tx))
            .is_ok()
        {
            if shutdown_rx.recv_timeout(Duration::from_secs(3)).is_ok() {
                Ok(())
            } else {
                error!("Serial worker didn't react to shutdown request.");
                Err(())
            }
        } else {
            error!("Couldn't send serial worker shutdown.");
            Err(())
        }
    }
}
