use std::{sync::Arc, thread::JoinHandle, time::Duration};

use arc_swap::ArcSwap;
use bstr::ByteVec;
use crossbeam::channel::{Receiver, Sender, bounded};
use serialport::SerialPortInfo;
use tracing::{debug, error};

use crate::{
    app::Event,
    serial::Reconnections,
    settings::{Ignored, PortSettings},
};

#[cfg(feature = "espflash")]
use super::esp::EspCommand;

use super::worker::{PortStatus, SerialWorker};

#[derive(Debug)]
pub enum SerialWorkerCommand {
    RequestPortScan,
    BlockingConnect {
        port: SerialPortInfo,
        baud: Option<u32>,
        settings: PortSettings,
        result_tx: Sender<Result<(), super::worker::WorkerError>>,
    },
    PortCommand(PortCommand),
    RequestReconnect(Option<Reconnections>),
    Disconnect {
        user_wants_break: bool,
    },
    NewIgnored(Ignored),
    Shutdown(Sender<()>),
}

#[derive(Debug)]
pub enum PortCommand {
    PortSettings(PortSettings),
    TxBuffer(Vec<u8>),
    #[cfg(feature = "espflash")]
    Esp(EspCommand),
    WriteSignals {
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    ToggleSignals {
        dtr: bool,
        rts: bool,
    },
}

type HandleResult<T> = Result<T, SerialWorkerMissing>;
#[derive(Debug, thiserror::Error)]
#[error("serial worker rx handle dropped")]
pub struct SerialWorkerMissing;
impl<T> From<crossbeam::channel::SendError<T>> for SerialWorkerMissing {
    fn from(_: crossbeam::channel::SendError<T>) -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct SerialHandle {
    pub(super) command_tx: Sender<SerialWorkerCommand>,
    pub port_status: Arc<ArcSwap<PortStatus>>,
    pub port_settings: Arc<ArcSwap<PortSettings>>,
}

impl SerialHandle {
    pub fn build(
        event_tx: Sender<Event>,
        buffer_tx: Sender<Vec<u8>>,
        port_settings: PortSettings,
        ignored_devices: Ignored,
    ) -> HandleResult<(Self, JoinHandle<()>)> {
        let (command_tx, command_rx) = crossbeam::channel::unbounded();

        let port_status = Arc::new(ArcSwap::from_pointee(PortStatus::new_idle(&port_settings)));

        let port_settings = Arc::new(ArcSwap::from_pointee(port_settings));

        let mut worker = SerialWorker::new(
            command_rx,
            event_tx,
            buffer_tx,
            port_status.clone(),
            port_settings.clone(),
            ignored_devices,
        );

        let worker = std::thread::spawn(move || {
            if let Err(e) = worker.work_loop() {
                error!("Serial worker closed with error: {e}");
            } else {
                debug!("Serial worker closed gracefully!");
            }
        });

        let handle = Self {
            command_tx,
            port_status,
            port_settings,
        };
        // Trigger first port scan before scheduled event to fill it in sooner
        handle.request_port_scan()?;
        Ok((handle, worker))
    }
    // pub fn connect_nonblocking(
    //     &self,
    //     port: &SerialPortInfo,
    //     settings: PortSettings,
    // ) -> HandleResult<()> {
    //     self.command_tx
    //         .send(SerialWorkerCommand::NonBlockingConnect {
    //             port: port.to_owned(),
    //             settings,
    //         })?;
    //     Ok(())
    // }
    pub fn connect_blocking(
        &self,
        port: SerialPortInfo,
        settings: PortSettings,
        baud: Option<u32>,
    ) -> HandleResult<Receiver<Result<(), super::worker::WorkerError>>> {
        let (oneshot_tx, oneshot_rx) = bounded(0);

        self.command_tx.send(SerialWorkerCommand::BlockingConnect {
            port,
            baud,
            settings,
            result_tx: oneshot_tx,
        })?;

        Ok(oneshot_rx)
    }
    pub fn request_break_connection(&self) -> HandleResult<()> {
        self.command_tx.send(SerialWorkerCommand::Disconnect {
            user_wants_break: true,
        })?;
        Ok(())
    }
    pub fn request_disconnect(&self) -> HandleResult<()> {
        self.command_tx.send(SerialWorkerCommand::Disconnect {
            user_wants_break: false,
        })?;
        Ok(())
    }
    pub fn update_settings(&self, settings: PortSettings) -> HandleResult<()> {
        self.command_tx
            .send(SerialWorkerCommand::PortCommand(PortCommand::PortSettings(
                settings,
            )))?;
        Ok(())
    }
    /// Sends the supplied bytes through the connected Serial device.
    pub fn send_bytes(&self, mut input: Vec<u8>, line_ending: Option<&[u8]>) -> HandleResult<()> {
        if let Some(ending) = line_ending {
            input.extend(ending.iter());
        }
        self.command_tx
            .send(SerialWorkerCommand::PortCommand(PortCommand::TxBuffer(
                input,
            )))?;
        Ok(())
    }
    pub fn send_str(
        &self,
        input: &str,
        line_ending: &[u8],
        unescape_bytes: bool,
    ) -> HandleResult<()> {
        // debug!("Outputting to serial: {input}");
        let buffer = if unescape_bytes {
            Vec::unescape_bytes(input)
        } else {
            input.as_bytes().to_owned()
        };
        self.send_bytes(buffer, Some(line_ending))
    }
    // pub fn read_signals(&self) -> WorkerResult<()> {
    //     self.command_tx
    //         .send(SerialCommand::ReadSignals)?;
    //     Ok(())
    // }
    pub fn write_signals(&self, dtr: Option<bool>, rts: Option<bool>) -> HandleResult<()> {
        self.command_tx.send(SerialWorkerCommand::PortCommand(
            PortCommand::WriteSignals { dtr, rts },
        ))?;
        Ok(())
    }
    pub fn toggle_signals(&self, dtr: bool, rts: bool) -> HandleResult<()> {
        self.command_tx.send(SerialWorkerCommand::PortCommand(
            PortCommand::ToggleSignals { dtr, rts },
        ))?;
        Ok(())
    }
    /// Non-blocking request for the serial worker to scan for ports and send a list of available ports
    pub fn request_port_scan(&self) -> HandleResult<()> {
        self.command_tx.send(SerialWorkerCommand::RequestPortScan)?;
        Ok(())
    }
    /// Non-blocking request for the serial worker to attempt to reconnect to the "current" device
    pub fn request_reconnect(&self, strictness_opt: Option<Reconnections>) -> HandleResult<()> {
        self.command_tx
            .send(SerialWorkerCommand::RequestReconnect(strictness_opt))?;
        Ok(())
    }
    /// Update the list of devices to not show in the Port Selection screen.
    pub fn new_ignored(&self, ignored: Ignored) -> HandleResult<()> {
        self.command_tx
            .send(SerialWorkerCommand::NewIgnored(ignored))?;
        Ok(())
    }

    /// Tells the worker thread to shutdown, blocking for up to three seconds before aborting.
    pub fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = crossbeam::channel::bounded(0);
        if self
            .command_tx
            .send(SerialWorkerCommand::Shutdown(shutdown_tx))
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
