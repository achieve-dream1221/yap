use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;
use crossbeam::channel::{Receiver, Sender};
use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use tracing::{debug, error, info, warn};
use virtual_serialport::VirtualPort;

use crate::{
    app::{Event, Tick},
    serial::{SerialDisconnectReason, SerialEvent},
    settings::{Ignored, PortSettings},
    traits::ToggleBool,
};

use super::{
    ReconnectType, Reconnections, SerialSignals,
    handle::{PortCommand, SerialWorkerCommand},
};

#[cfg(feature = "espflash")]
use super::esp::{EspCommand, EspEvent};
#[cfg(feature = "espflash")]
use espflash::connection::reset::ResetStrategy;

#[cfg(unix)]
pub type NativePort = serialport::TTYPort;
#[cfg(windows)]
pub type NativePort = serialport::COMPort;

#[derive(Default, strum::EnumIs)]
enum TakeablePort {
    #[default]
    None,
    Borrowed,
    Native(NativePort),
    Loopback(VirtualPort),
}

impl TakeablePort {
    fn is_some(&self) -> bool {
        !self.is_none()
    }
    fn is_available(&self) -> bool {
        match self {
            TakeablePort::Native(_) | TakeablePort::Loopback(_) => true,
            _ => false,
        }
    }
    fn drop(&mut self) {
        *self = TakeablePort::None;
    }
    fn take_native(&mut self) -> Option<NativePort> {
        if let TakeablePort::Native(_) = self {
            if let TakeablePort::Native(port) = std::mem::replace(self, TakeablePort::Borrowed) {
                Some(port)
            } else {
                unreachable!()
            }
        } else {
            None
        }
    }
    fn return_native(&mut self, port: NativePort) {
        *self = TakeablePort::Native(port);
    }
    fn return_loopback(&mut self, port: VirtualPort) {
        *self = TakeablePort::Loopback(port);
    }
    fn as_mut_port(&mut self) -> Option<&mut dyn SerialPort> {
        match self {
            TakeablePort::Native(port) => Some(port),
            TakeablePort::Loopback(port) => Some(port),
            _ => None,
        }
    }
    fn as_mut_native_port(&mut self) -> Option<&mut NativePort> {
        if let TakeablePort::Native(port) = self {
            Some(port)
        } else {
            None
        }
    }
}

pub struct SerialWorker {
    command_rx: Receiver<SerialWorkerCommand>,
    event_tx: Sender<Event>,
    buffer_tx: Sender<Vec<u8>>,
    port: TakeablePort,
    last_signal_check: Instant,
    // settings: PortSettings,
    scan_snapshot: Vec<SerialPortInfo>,
    rx_buffer: Vec<u8>,
    shared_status: Arc<ArcSwap<PortStatus>>,
    shared_settings: Arc<ArcSwap<PortSettings>>,
    ignored_devices: Ignored,
}

impl SerialWorker {
    pub fn new(
        command_rx: Receiver<SerialWorkerCommand>,
        event_tx: Sender<Event>,
        buffer_tx: Sender<Vec<u8>>,
        port_status: Arc<ArcSwap<PortStatus>>,
        port_settings: Arc<ArcSwap<PortSettings>>,
        ignored_devices: Ignored,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            buffer_tx,
            shared_status: port_status,
            shared_settings: port_settings,
            // port_status: SerialStatus::idle(),
            // connected_port_info: None,
            port: TakeablePort::default(),
            last_signal_check: Instant::now(),
            // settings: PortSettings::default(),
            scan_snapshot: vec![],
            rx_buffer: vec![0; 1024 * 1024],
            ignored_devices,
        }
    }
    // Primary loop for this thread.
    // Only use `?` on operations here that we expect to run flawlessly, and that
    // should kill the __whole app__ if encountered.
    pub fn work_loop(&mut self) -> Result<(), WorkerError> {
        loop {
            // TODO consider sleeping here for a moment with a read_timeout?
            // or have some kind of cooldown after a 0-size serial read
            // (maybe use port.bytes_to_read() ?)
            // not sure if the barrage is what's causing weird unix issues with the ESP32-S3, need to test further

            // TODO toy with the timeouts when writing to see if i can tell if im hitting the S3's ring buffer limit
            let sleep_time = if self.port.is_some() {
                Duration::from_millis(10)
            } else {
                Duration::from_millis(100)
            };
            // TODO Fuzz testing with this + buffer
            match self.command_rx.recv_timeout(sleep_time) {
                Ok(SerialWorkerCommand::Shutdown(shutdown_tx)) => {
                    if let Some(port) = self.port.as_mut_port() {
                        _ = port.flush();
                    }
                    self.port.drop();
                    self.shared_status
                        .store(Arc::new(PortStatus::new_idle(&PortSettings::default())));

                    if let Err(_) = shutdown_tx.send(()) {
                        error!("Failed to reply to shutdown request!");
                        break Err(WorkerError::ShutdownReply);
                    } else {
                        break Ok(());
                    }
                }
                Ok(SerialWorkerCommand::PortCommand(port_cmd)) => {
                    if let Err(e) = self.handle_port_command(port_cmd) {
                        error!("Port command error: {e}");
                        self.unhealthy_disconnection();
                        self.event_tx
                            .send(SerialDisconnectReason::Error(e.to_string()).into())?;
                    }
                }
                Ok(cmd) => self.handle_worker_command(cmd)?,

                // no message, just move on
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => (),
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                    error!("Serial worker handle got dropped! Shutting down!");
                    break Err(WorkerError::HandleDropped);
                }
            }

            if let Some(port) = self.port.as_mut_port() {
                // info!(
                //     "bytes incoming: {}, bytes outcoming: {}",
                //     port.bytes_to_read().unwrap(),
                //     port.bytes_to_write().unwrap()
                // );
                match port.read(self.rx_buffer.as_mut_slice()) {
                    Ok(t) if t > 0 => {
                        let cloned_buff = self.rx_buffer[..t].to_owned();
                        // info!("{:?}", &serial_buf[..t]);
                        self.buffer_tx.send(cloned_buff)?;
                    }
                    // 0-size read, ignoring
                    Ok(_) => (),

                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => (),
                    Err(e) => {
                        error!("{:?}", e);
                        self.unhealthy_disconnection();
                        self.event_tx
                            .send(SerialDisconnectReason::Error(e.to_string()).into())?;
                    }
                }

                if self.last_signal_check.elapsed() >= Duration::from_millis(100) {
                    self.last_signal_check = Instant::now();
                    if let Err(e) = self.read_and_share_serial_signals(false) {
                        error!("{:?}", e);
                        self.unhealthy_disconnection();
                        self.event_tx
                            .send(SerialDisconnectReason::Error(e.to_string()).into())?;
                    }
                }
            }
        }
    }

    fn unhealthy_disconnection(&mut self) {
        assert!(self.port.is_some(), "must own or be lending out port");

        let last_status = self.shared_status.load().as_ref().clone();
        let known_port_ref = last_status
            .current_port
            .as_ref()
            .expect("shouldn't have been connected to port without data saved");

        // Check if the port is still seen by the system
        if let Ok(mut latest_ports) = self.scan_for_serial_ports() {
            let current_port_index = latest_ports.iter().position(|s| s == known_port_ref);
            if let Some(idx) = current_port_index {
                // If so, remove it from the snapshot.
                latest_ports.remove(idx);
                self.scan_snapshot = latest_ports;
            } else {
                // If not, then we're (probably) safe to just use it as-is for
                // our "at-disconnect" snapshot to avoid already-connected
                // similar-looking USB devices triggering reconnections.
                self.scan_snapshot = latest_ports;
            }
        } else {
            error!("Failed to scan for ports right after a disconnection!")
        };

        self.port.drop();
        let disconnected_status = {
            last_status
                // Ensure we keep around the old SerialPortInfo to use
                // as a reference for reconnections!
                .to_unhealthy()
        };
        self.shared_status.store(Arc::new(disconnected_status));
        // Disconnection Event TX should be done by caller.
    }

    // Errors returned should be treated as fatal Worker errors.
    fn handle_worker_command(&mut self, command: SerialWorkerCommand) -> Result<(), WorkerError> {
        match command {
            SerialWorkerCommand::Connect { port, settings } => {
                // TODO figure out error flow with this and espflash stuff
                // maybe just put on top level like the others?
                self.update_settings(settings)?;
                self.connect_to_port(&port, None)?;
            }
            SerialWorkerCommand::Disconnect {
                user_wants_break: false,
            } => {
                // self.port_status = SerialStatus::idle();

                let settings = self.shared_settings.load();

                let previous_status = { self.shared_status.load().as_ref().clone() };

                self.shared_status
                    .store(Arc::new(previous_status.to_idle(&*settings)));
                self.port.drop();
                self.event_tx
                    .send(SerialDisconnectReason::Intentional.into())?;
            }

            SerialWorkerCommand::Disconnect {
                user_wants_break: true,
            } if self.port.is_some() => {
                self.unhealthy_disconnection();

                self.event_tx
                    .send(SerialDisconnectReason::UserBrokeConnection.into())?;
            }
            SerialWorkerCommand::Disconnect {
                user_wants_break: true,
            } => warn!("No owned port connection to break!"),

            SerialWorkerCommand::RequestPortScan => {
                let ports = self.scan_for_serial_ports()?;
                self.scan_snapshot = ports.clone();
                self.event_tx.send(SerialEvent::Ports(ports).into())?;
            }
            SerialWorkerCommand::RequestReconnect(strictness_opt) => {
                if let Err(e) = self.attempt_reconnect(strictness_opt) {
                    // TODO maybe show on UI?
                    error!("Failed reconnect attempt: {e}");
                }
            }
            SerialWorkerCommand::Shutdown(_) => unreachable!(),
            SerialWorkerCommand::PortCommand(_) => unreachable!(),
        }
        Ok(())
    }
    // Errors returned should break existing port connection,
    // and begin reconnect attempts (if allowed)
    fn handle_port_command(&mut self, command: PortCommand) -> Result<(), WorkerError> {
        match command {
            PortCommand::PortSettings(settings) => self.update_settings(settings)?,
            #[cfg(feature = "espflash")]
            PortCommand::Esp(esp_command) => self.handle_esp_command(esp_command)?,

            PortCommand::WriteSignals { dtr, rts } => {
                assert!(dtr.is_some() || rts.is_some());
                let mut status: PortStatus = self.shared_status.load().as_ref().clone();
                if let Some(dtr) = dtr {
                    status.signals.dtr = dtr;
                }
                if let Some(rts) = rts {
                    status.signals.rts = rts;
                }
                if let Some(port) = self.port.as_mut_port() {
                    // Sending both signals regardless of which one changed
                    // to keep them in line with the expected state in the struct.
                    port.write_data_terminal_ready(status.signals.dtr)?;
                    port.write_request_to_send(status.signals.rts)?;
                }
                self.shared_status.store(Arc::new(status));
                self.read_and_share_serial_signals(true)?;
            }
            PortCommand::ToggleSignals { dtr, rts } => {
                assert!(dtr || rts);

                let mut status: PortStatus = self.shared_status.load().as_ref().clone();

                if dtr {
                    status.signals.dtr.flip();
                }
                if rts {
                    status.signals.rts.flip();
                }
                if let Some(port) = self.port.as_mut_port() {
                    // Sending both signals regardless of which one changed
                    // to keep them in line with the expected state in the struct.
                    port.write_data_terminal_ready(status.signals.dtr)?;
                    port.write_request_to_send(status.signals.rts)?;
                }
                self.shared_status.store(Arc::new(status));
                self.read_and_share_serial_signals(true)?;
            }
            // This should maybe reply with a success/fail in case the
            // port is having an issue, so the user's input buffer isn't consumed visually
            PortCommand::TxBuffer(data) if self.port.is_available() => {
                let port = self
                    .port
                    .as_mut_port()
                    .expect("was told port was available");

                let mut buf = &data[..];

                // let mut writer = BufWriter::new(&mut port);

                // TODO This is because the ESP32-S3's virtual USB serial port
                // has an issue with payloads larger than 256 bytes????
                // (Sending too fast causes the buffer to fill up too quickly for the
                // actual firmware to notice anything present and drain it before it hits the cap)
                // So this might need to be a throttle toggle,
                // maybe on by default since its not too bad?
                let slow_writes = true;

                let max_bytes = 8;

                while !buf.is_empty() {
                    let write_size = if slow_writes {
                        std::cmp::min(max_bytes, buf.len())
                    } else {
                        buf.len()
                    };
                    match port.write(&buf[..write_size]) {
                        Ok(0) => {
                            // return Err(Error::WRITE_ALL_EOF);
                            todo!("serialport write returned Ok(0)???");
                        }
                        Ok(n) => {
                            // info!("buf n: {n}");
                            buf = &buf[n..];
                            self.event_tx.send(Tick::Tx.into())?;
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(e) => {
                            self.unhealthy_disconnection();
                            self.event_tx
                                .send(SerialDisconnectReason::Error(e.to_string()).into())?;
                            return Ok(());
                        }
                    }
                }
            }
            // Tried to send with no port
            PortCommand::TxBuffer(unsent) => {
                let len = unsent.len();
                warn!("Got a buffer of {len} len that can't be sent! Returning buffer...");
                self.event_tx.send(SerialEvent::UnsentTx(unsent).into())?;
            }
        }
        Ok(())
    }
    fn update_settings(&mut self, settings: PortSettings) -> Result<(), serialport::Error> {
        let status = { self.shared_status.load().as_ref().clone() };
        if let Some(port) = self.port.as_mut_port() {
            port.set_baud_rate(settings.baud_rate)?;
            port.set_parity(settings.parity_bits)?;
            port.set_stop_bits(settings.stop_bits)?;
            port.set_data_bits(settings.data_bits)?;
            port.set_flow_control(settings.flow_control)?;

            port.write_data_terminal_ready(status.signals.dtr)?;
            port.write_request_to_send(status.signals.rts)?;
        } else {
            warn!("Received new port settings when no port connected!");
        }
        self.shared_status.store(Arc::new(status));
        self.shared_settings.store(Arc::new(settings));
        Ok(())
    }

    /// A return value of `Ok(())` only means no errors were encountered,
    /// not that reconnection was successful.
    fn attempt_reconnect(
        &mut self,
        strictness_opt: Option<Reconnections>,
    ) -> Result<(), WorkerError> {
        // assert!(self.connected_port_info.read().unwrap().is_some());
        // assert!(self.port.is_none());
        if self.port.is_available() || self.port.is_borrowed() {
            error!("Got request to reconnect when already connected to port! Not acting...");
            return Ok(());
        }

        let reconnections =
            strictness_opt.unwrap_or_else(|| self.shared_settings.load().reconnections.clone());

        if reconnections == Reconnections::Disabled {
            warn!("Got request to reconnect when reconnections are disabled!");
            return Ok(());
        }

        let current_ports = self.scan_for_serial_ports()?;
        let port_guard = self.shared_status.load();
        let desired_port = port_guard
            .current_port
            .as_ref()
            .expect("shouldn't have been connected to port without data saved");

        // Checking for a perfect match
        // TODO look into the USB Interface field for SerialPortInfo, see if we should keep it in mind
        // for the potential extra+less strict check
        if let Some(port) = current_ports.iter().find(|p| *p == desired_port) {
            info!("Perfect match found! Reconnecting to: {}", port.port_name);
            // Sleeping to give the device some time to intialize with Windows
            // (Otherwise Access Denied errors can occur from trying to connect too quick)
            std::thread::sleep(Duration::from_secs(1));
            self.connect_to_port(&port, Some(ReconnectType::PerfectMatch))?;
            return Ok(());
        };

        // Fuzzy USB searches
        if let SerialPortType::UsbPort(desired_usb) = &desired_port.port_type {
            // Strict check, trying to find a *new* port that has all the same USB characteristics
            if let Some(port) = current_ports
                .iter()
                // Only searching for USB Serial port devices
                .filter(|p| matches!(p.port_type, SerialPortType::UsbPort(_)))
                // Filtering out ports that didn't change across scans
                // (so we don't connect to an identical device that was already present and not being used)
                .filter(|p| !self.scan_snapshot.contains(p))
                .find(|p| p.port_type == desired_port.port_type)
            {
                info!(
                    "[STRICT] Connecting to similar USB device with port: {}",
                    port.port_name
                );
                std::thread::sleep(Duration::from_secs(1));
                self.connect_to_port(&port, Some(ReconnectType::UsbStrict))?;
                return Ok(());
            };

            // Loose check
            if reconnections == Reconnections::LooseChecks {
                if let Some(port) = current_ports
                    .iter()
                    // Filtering out ports that didn't change across scans
                    .filter(|p| !self.scan_snapshot.contains(p))
                    // Trying to find another USB device with *just* matching USB PID & VID
                    // Use cases: Some devices seem to change their Serial # arbitrarily?
                    //          - And for interfacing with several identical devices (one at a time) without reconnecting via TUI
                    // Needs a toggle with Strict/Loose options, as the extra behavior isn't always desirable.
                    .find(|p| match &p.port_type {
                        SerialPortType::UsbPort(usb) => {
                            usb.vid == desired_usb.vid && usb.pid == desired_usb.pid
                        }
                        _ => false,
                    })
                {
                    info!(
                        "[NON-STRICT] Connecting to similar USB device with port: {}",
                        port.port_name
                    );
                    std::thread::sleep(Duration::from_secs(1));
                    self.connect_to_port(&port, Some(ReconnectType::UsbLoose))?;
                    return Ok(());
                };
            }
        }

        // Loose check
        if reconnections == Reconnections::LooseChecks {
            // Last ditch effort, just try to connect to the same port_name if it's present.
            if let Some(port) = current_ports
                .iter()
                .find(|p| *p.port_name == desired_port.port_name)
            {
                info!("Last ditch connect attempt on: {}", port.port_name);
                std::thread::sleep(Duration::from_secs(1));
                self.connect_to_port(&port, Some(ReconnectType::LastDitch))?;
                return Ok(());
            }
        }

        // {}

        Ok(())
    }

    fn scan_for_serial_ports(&self) -> Result<Vec<SerialPortInfo>, serialport::Error> {
        // TODO error handling
        let mut ports = serialport::available_ports()?;
        ports.push(SerialPortInfo {
            port_name: MOCK_PORT_NAME.to_owned(),
            port_type: SerialPortType::Unknown,
        });

        // ports
        //     .iter()
        //     .map(|p| match &p.port_type {
        //         SerialPortType::UsbPort(usb) => info!("{usb:#?}"),
        //         _ => (),
        //     })
        //     .count();

        ports.retain(|p| match &p.port_type {
            // Hardcoded filter for Index/Beyond's Bluetooth COM Port
            // TODO remove when releasing?
            SerialPortType::UsbPort(usb) if usb.vid == 0x28DE && usb.pid == 0x2102 => false,

            SerialPortType::UsbPort(usb) => !self.ignored_devices.usb.iter().any(|ig| ig == usb),
            // TODO filters for other types
            _ => true,
        });

        // TODO: Add filters for this in UI
        #[cfg(unix)]
        ports.retain(|port| {
            !(port.port_type == SerialPortType::Unknown && !port.port_name.eq(MOCK_PORT_NAME))
        });

        // info!("Serial port scanning found {} ports", ports.len());
        // self.scanned_ports = ports;
        Ok(ports)
    }
    fn connect_to_port(
        &mut self,
        port_info: &SerialPortInfo,
        reconnect_type: Option<ReconnectType>,
    ) -> Result<(), WorkerError> {
        let mut port_status: PortStatus = self.shared_status.load().as_ref().clone();
        // If this is a normal connection, then this should be set to settings.dtr_on_open
        // otherwise, if we're reconnecting, then this should match the state of DTR at the time of disconnection
        let dtr_on_open = port_status.signals.dtr;
        let settings = self.shared_settings.load();
        let baud_rate = settings.baud_rate;

        if port_info.port_name.eq(MOCK_PORT_NAME) {
            let mut virt_port =
                virtual_serialport::VirtualPort::loopback(baud_rate, MOCK_DATA.len() as u32)?;
            virt_port.write_all(MOCK_DATA)?;

            self.port.return_loopback(virt_port);
        } else {
            let port = serialport::new(&port_info.port_name, baud_rate)
                .data_bits(settings.data_bits)
                .flow_control(settings.flow_control)
                .parity(settings.parity_bits)
                .stop_bits(settings.stop_bits)
                .dtr_on_open(dtr_on_open)
                .open_native()?;

            self.port.return_native(port);
        };

        let port = self
            .port
            .as_mut_port()
            .expect("port just populated, should be present");
        port.set_timeout(Duration::from_millis(100))?;
        port.write_request_to_send(port_status.signals.rts)?;

        port_status.signals.update_with_port(port)?;
        port_status.current_port = Some(port_info.to_owned());
        port_status.inner = InnerPortStatus::Connected;
        self.shared_status.store(Arc::new(port_status));

        info!(
            "Serial worker connected to: {} @ {baud_rate} baud",
            port_info.port_name
        );
        // info!("port.baud_rate {}", self.port.as_ref().unwrap().baud_rate()?);
        self.event_tx
            .send(SerialEvent::Connected(reconnect_type).into())?;
        Ok(())
    }

    fn read_and_share_serial_signals(&mut self, force_share: bool) -> Result<(), WorkerError> {
        let mut port_status: PortStatus = self.shared_status.load().as_ref().clone();

        match self.port.as_mut_port() {
            // If no port is present, just skip.
            None => return Ok(()),
            Some(port) => {
                // debug_assert later?
                // assert!(self.baud_rate == port_status.baud_rate);

                let changed = port_status.signals.update_with_port(port)?;
                // Only update the shared status if there's actually a change
                if changed {
                    self.shared_status.store(Arc::new(port_status));
                }
                // But always send the UI update tick if a DTR/RTS change requested it
                if changed || force_share {
                    self.event_tx
                        .send(Tick::Requested("Serial Signals").into())?;
                }
            }
        }

        Ok(())
    }

    #[cfg(feature = "espflash")]
    // Returning an error from here means that we couldn't recover,
    // and the connection needs to be re-established.
    fn handle_esp_command(&mut self, esp_command: EspCommand) -> Result<(), WorkerError> {
        if !self.port.is_native() {
            error!("ESP Command given when we don't own native port!");
            return Err(WorkerError::MissingPort);
        }

        use std::{borrow::Cow, fs};

        use compact_str::{CompactString, ToCompactString};
        use espflash::{
            elf::RomSegment,
            flasher::{FlashData, FlashSettings, ProgressCallbacks},
        };
        use serialport::UsbPortInfo;

        use crate::{
            serial::esp::{EspRestartType, FlashProgress, ProgressPropagator},
            tui::esp::EspProfile,
        };

        let mut status: PortStatus = self.shared_status.load().as_ref().clone();

        let usb_port_info = {
            match &status.current_port {
                None => unreachable!(),
                Some(info) => match &info.port_type {
                    SerialPortType::UsbPort(e) => e.clone(),
                    _not_usb => UsbPortInfo {
                        vid: 0,
                        pid: 0,
                        serial_number: None,
                        manufacturer: None,
                        product: None,
                    },
                },
            }
        };

        status.inner = InnerPortStatus::LentOut;

        self.shared_status.store(Arc::new(status.clone()));

        self.event_tx.send(EspEvent::Connecting.into())?;

        let lent_port = self
            .port
            .take_native()
            .expect("worker should have native port ownership");

        let returned_port = match esp_command {
            EspCommand::DeviceInfo => {
                let mut flasher = self.connect_esp_flasher(lent_port, usb_port_info, true, true)?;

                if let Ok(esp_info) = flasher.device_info() {
                    debug!("{esp_info:#?}");

                    self.event_tx.send(EspEvent::DeviceInfo(esp_info).into())?;
                }

                flasher.connection().reset()?;

                flasher.into_serial()
            }

            EspCommand::Restart(restart_type) => match restart_type {
                EspRestartType::Bootloader { active: true } => {
                    let flasher = self.connect_esp_flasher(lent_port, usb_port_info, true, true)?;

                    self.event_tx.send(
                        EspEvent::BootloaderSuccess {
                            chip: flasher.chip().to_compact_string().to_uppercase(),
                        }
                        .into(),
                    )?;

                    flasher.into_serial()
                }
                EspRestartType::Bootloader { active: false } => {
                    let mut connection = espflash::connection::Connection::new(
                        lent_port,
                        usb_port_info,
                        espflash::connection::reset::ResetAfterOperation::HardReset,
                        espflash::connection::reset::ResetBeforeOperation::DefaultReset,
                    );

                    connection.reset_to_flash(true)?;

                    self.event_tx.send(EspEvent::HardResetAttempt.into())?;

                    connection.into_serial()
                }
                EspRestartType::UserCode => {
                    let mut connection = espflash::connection::Connection::new(
                        lent_port,
                        usb_port_info,
                        espflash::connection::reset::ResetAfterOperation::HardReset,
                        espflash::connection::reset::ResetBeforeOperation::DefaultReset,
                    );

                    connection.reset()?;

                    self.event_tx.send(EspEvent::HardResetAttempt.into())?;

                    connection.into_serial()
                }
            },
            EspCommand::FlashProfile(EspProfile::Bins(bins)) => {
                assert!(!bins.bins.is_empty(), "expected at least one bin to flash");
                let mut flasher = self.connect_esp_flasher(
                    lent_port,
                    usb_port_info,
                    !bins.no_verify,
                    !bins.no_skip,
                )?;

                let matches = bins.expected_chip.map_or(true, |expected| {
                    if expected == flasher.chip() {
                        true
                    } else {
                        warn!("Not acting! Chip doesn't match!");
                        false
                    }
                });

                if matches {
                    use itertools::Itertools;

                    if let Some(baud) = bins.upload_baud {
                        flasher.change_baud(baud)?;
                    }

                    let (rom_segs, mut errs): (Vec<RomSegment>, Vec<std::io::Error>) = bins
                        .bins
                        .iter()
                        .map(|(addr, path)| -> Result<RomSegment, std::io::Error> {
                            let bytes = fs::read(path)?;
                            Ok(RomSegment {
                                addr: *addr,
                                data: Cow::Owned(bytes),
                            })
                        })
                        .partition_result();

                    if let Some(err) = errs.pop() {
                        return Err(err)?;
                    }

                    let filenames: Vec<_> = bins
                        .bins
                        .iter()
                        .map(|(_addr, path)| path.file_name().unwrap_or_default())
                        .collect();
                    if filenames.iter().any(|n| n.is_empty()) {
                        return Err(WorkerError::FileMissingName);
                    }

                    let mut propagator = ProgressPropagator::new(
                        self.event_tx.clone(),
                        flasher.chip().to_compact_string().to_uppercase(),
                        filenames,
                    );

                    if let Err(e) = flasher.write_bins_to_flash(&rom_segs, Some(&mut propagator)) {
                        // TODO show on UI
                        self.event_tx
                            .send(EspEvent::Error(format!("espflash error: {e}")).into())?;
                        error!("Error during flashing: {e}");
                    }

                    self.event_tx.send(EspEvent::PortReturned.into())?;
                } else {
                    self.event_tx.send(
                        EspEvent::Error(
                            "Not flashing! ESP variant doesn't match expected!".to_owned(),
                        )
                        .into(),
                    )?;
                }

                flasher.into_serial()
            }
            EspCommand::FlashProfile(EspProfile::Elf(elf)) => {
                // assert!(!elf.path.is_empty(), "expected path");
                let mut flasher = self.connect_esp_flasher(
                    lent_port,
                    usb_port_info,
                    !elf.no_verify,
                    !elf.no_skip,
                )?;

                let matches = elf.expected_chip.map_or(true, |expected| {
                    if expected == flasher.chip() {
                        true
                    } else {
                        warn!("Not acting! Chip doesn't match!");
                        false
                    }
                });

                if matches {
                    if let Some(baud) = elf.upload_baud {
                        flasher.change_baud(baud)?;
                    }

                    let elf_data = fs::read(&elf.path)?;

                    let mut propagator = ProgressPropagator::new(
                        self.event_tx.clone(),
                        flasher.chip().to_compact_string().to_uppercase(),
                        vec![],
                    );

                    if elf.ram {
                        if let Err(e) = flasher.load_elf_to_ram(&elf_data, Some(&mut propagator)) {
                            // TODO show on UI
                            self.event_tx
                                .send(EspEvent::Error(format!("espflash error: {e}")).into())?;
                            error!("Error during RAM load: {e}");
                        }
                    } else {
                        if let Ok(esp_info) = flasher.device_info() {
                            let flash_data = FlashData::new(
                                elf.bootloader.as_ref().map(AsRef::as_ref),
                                elf.partition_table.as_ref().map(AsRef::as_ref),
                                // TODO?
                                None,
                                // TODO?
                                None,
                                FlashSettings::default(),
                                0,
                            );

                            let flash_data = match flash_data {
                                Ok(data) => data,
                                Err(e) => Err(WorkerError::FlashData(e.to_string()))?,
                            };

                            if let Err(e) = flasher.load_elf_to_flash(
                                &elf_data,
                                flash_data,
                                Some(&mut propagator),
                                esp_info.crystal_frequency,
                            ) {
                                // TODO show on UI
                                self.event_tx
                                    .send(EspEvent::Error(format!("espflash error: {e}")).into())?;
                                error!("Error during flashing: {e}");
                            }
                        }
                    }

                    self.event_tx.send(EspEvent::PortReturned.into())?;
                } else {
                    self.event_tx.send(
                        EspEvent::Error(
                            "Not flashing! ESP variant doesn't match expected!".to_owned(),
                        )
                        .into(),
                    )?;
                }

                flasher.into_serial()
            }
            EspCommand::EraseFlash => {
                let mut flasher = self.connect_esp_flasher(lent_port, usb_port_info, true, true)?;
                let esp_chip = flasher.chip();
                let chip = esp_chip.to_compact_string().to_uppercase();

                self.event_tx
                    .send(EspEvent::EraseStart { chip: chip.clone() }.into())?;

                if let Ok(()) = flasher.erase_flash() {
                    self.event_tx.send(EspEvent::EraseSuccess { chip }.into())?;
                }

                flasher.into_serial()
            }
        };

        self.return_native_port(returned_port)?;
        self.event_tx.send(EspEvent::PortReturned.into())?;

        Ok(())
    }

    fn return_native_port(&mut self, mut port: NativePort) -> Result<(), WorkerError> {
        let mut status = self.shared_status.load().as_ref().clone();

        port.set_timeout(Duration::from_millis(100))?;

        let baud_rate = self.shared_settings.load().baud_rate;
        port.set_baud_rate(baud_rate)?;
        port.write_data_terminal_ready(status.signals.dtr)?;
        port.write_request_to_send(status.signals.rts)?;

        self.port.return_native(port);

        status.inner = InnerPortStatus::Connected;

        self.shared_status.store(Arc::new(status));

        self.event_tx.send(SerialEvent::Connected(None).into())?;

        Ok(())
    }
    #[cfg(feature = "espflash")]
    fn connect_esp_flasher(
        &self,
        lent_port: NativePort,
        usb_port_info: serialport::UsbPortInfo,
        verify: bool,
        skip: bool,
    ) -> Result<espflash::flasher::Flasher, WorkerError> {
        use compact_str::ToCompactString;

        let flasher = espflash::flasher::Flasher::connect(
            lent_port,
            usb_port_info,
            Some(115200),
            true,
            verify,
            skip,
            None,
            espflash::connection::reset::ResetAfterOperation::HardReset,
            espflash::connection::reset::ResetBeforeOperation::DefaultReset,
        )?;

        let esp_chip = flasher.chip();
        let chip = esp_chip.to_compact_string().to_uppercase();

        self.event_tx.send(EspEvent::Connected { chip }.into())?;

        Ok(flasher)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkerError {
    #[error("serial port error: {0}")]
    SerialPort(#[from] serialport::Error),
    #[error("no parent app reciever to send to")]
    FailedSend,
    #[error("failed to reply to shutdown request in time")]
    ShutdownReply,
    #[error("handle dropped, can't recieve commands")]
    HandleDropped,

    #[cfg(feature = "espflash")]
    #[error("espflash error: {0}")]
    EspFlash(#[from] espflash::error::Error),

    #[cfg(feature = "espflash")]
    #[error("file error: {0}")]
    File(#[from] std::io::Error),

    #[cfg(feature = "espflash")]
    #[error("given file has invalid name")]
    FileMissingName,

    #[cfg(feature = "espflash")]
    #[error("failed creating FlashData: {0}")]
    FlashData(String),

    #[cfg(feature = "espflash")]
    #[error("tried to act on lent out port")]
    MissingPort,
}

impl<T> From<crossbeam::channel::SendError<T>> for WorkerError {
    fn from(_: crossbeam::channel::SendError<T>) -> Self {
        Self::FailedSend
    }
}

// This status struct leaves a bit to be desired
// especially in terms of the signals and their initial states
// (between connections and app start)
// maybe something better will come to me.

#[derive(Debug, Clone, Copy, Default)]
pub enum InnerPortStatus {
    #[default]
    Idle,
    PrematureDisconnect,
    LentOut,
    Connected,
}

impl InnerPortStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, InnerPortStatus::Connected)
    }
    pub fn is_lent_out(&self) -> bool {
        matches!(self, InnerPortStatus::LentOut)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PortStatus {
    pub inner: InnerPortStatus,
    pub current_port: Option<SerialPortInfo>,

    pub signals: SerialSignals,
}

impl PortStatus {
    pub fn new_idle(settings: &PortSettings) -> Self {
        Self {
            signals: SerialSignals {
                dtr: settings.dtr_on_connect,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    /// Used when a port disconnects without the user's stated intent to do so.
    fn to_unhealthy(self) -> Self {
        Self {
            inner: InnerPortStatus::PrematureDisconnect,
            ..self
        }
    }
    /// Used when the user chooses to disconnect from the serial port
    fn to_idle(self, settings: &PortSettings) -> Self {
        Self {
            inner: InnerPortStatus::Idle,
            current_port: None,
            signals: SerialSignals {
                dtr: settings.dtr_on_connect,
                ..Default::default()
            },
            ..self
        }
    }

    // fn to_connected(
    //     self,
    //     port: SerialPortInfo, // , baud_rate: u32, signals: SerialSignals
    // ) -> Self {
    //     Self {
    //         healthy: true,
    //         current_port: Some(port),
    //         ..self
    //     }
    // }
}

pub const MOCK_PORT_NAME: &str = "lorem-ipsum";

const MOCK_DATA: &[u8] = b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1A\x1B\x1C\x1D\x1E\x1F\x20\x21\x22\x23\x24\x25\x26\x27\x28\x29\x2A\x2B\x2C\x2D\x2E\x2F\x30\x31\x32\x33\x34\x35\x36\x37\x38\x39\x3A\x3B\x3C\x3D\x3E\x3F\x40\x41\x42\x43\x44\x45\x46\x47\x48\x49\x4A\x4B\x4C\x4D\x4E\x4F\x50\x51\x52\x53\x54\x55\x56\x57\x58\x59\x5A\x5B\x5C\x5D\x5E\x5F\x60\x61\x62\x63\x64\x65\x66\x67\x68\x69\x6A\x6B\x6C\x6D\x6E\x6F\x70\x71\x72\x73\x74\x75\x76\x77\x78\x79\x7A\x7B\x7C\x7D\x7E\x7F\x80\x81\x82\x83\x84\x85\x86\x87\x88\x89\x8A\x8B\x8C\x8D\x8E\x8F\x90\x91\x92\x93\x94\x95\x96\x97\x98\x99\x9A\x9B\x9C\x9D\x9E\x9F\xA0\xA1\xA2\xA3\xA4\xA5\xA6\xA7\xA8\xA9\xAA\xAB\xAC\xAD\xAE\xAF\xB0\xB1\xB2\xB3\xB4\xB5\xB6\xB7\xB8\xB9\xBA\xBB\xBC\xBD\xBE\xBF\xC0\xC1\xC2\xC3\xC4\xC5\xC6\xC7\xC8\xC9\xCA\xCB\xCC\xCD\xCE\xCF\xD0\xD1\xD2\xD3\xD4\xD5\xD6\xD7\xD8\xD9\xDA\xDB\xDC\xDD\xDE\xDF\xE0\xE1\xE2\xE3\xE4\xE5\xE6\xE7\xE8\xE9\xEA\xEB\xEC\xED\xEE\xEF\xF0\xF1\xF2\xF3\xF4\xF5\xF6\xF7\xF8\xF9\xFA\xFB\xFC\xFD\xFE\xFF";

/*
const MOCK_DATA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Duis porta volutpat magna non suscipit. Fusce rhoncus placerat metus, in posuere elit porta eget. Praesent ut nulla euismod, pulvinar tellus a, interdum ipsum. Integer in risus vulputate, finibus sem a, mattis ipsum. Aenean nec hendrerit tellus. Fusce risus dolor, sagittis non libero tristique, mattis vulputate libero. Proin ultrices luctus malesuada. Vestibulum non condimentum augue. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Vestibulum ultricies quis neque non pharetra. Nam fringilla nisl at tortor malesuada cursus. Nulla dictum, sem ac dignissim ullamcorper, est purus interdum tellus, at sagittis arcu risus suscipit neque. Mauris varius mauris vitae mi sollicitudin eleifend.

Donec feugiat, arcu sit amet ullamcorper consequat, nibh dolor laoreet risus, ut tincidunt tortor felis sed lacus. Aenean facilisis, mi nec feugiat rhoncus, dui urna malesuada erat, id mollis ipsum lectus ut ex. Curabitur semper vel tortor in finibus. Maecenas elit dui, cursus condimentum venenatis nec, cursus eget nisl. Proin consequat rhoncus tempor. Etiam dictum purus erat, sed aliquam mauris euismod vitae. Vivamus ut eros varius, posuere dolor eget, pretium tellus. Nam non lorem quis massa luctus hendrerit. Phasellus lobortis sodales quam in scelerisque. Morbi euismod et enim id dignissim. Sed commodo purus non est pellentesque euismod. Donec tincidunt dolor a ante aliquam auctor. Nam eget blandit felis.

Curabitur in tincidunt nunc. Phasellus in metus est. Nulla facilisi. Mauris dapibus augue non urna efficitur, eu ultrices est pellentesque. Nam semper vel nisi a pretium. Aenean malesuada sagittis mi, sit amet tempor mi. Donec at bibendum felis. Mauris a tortor luctus, tincidunt dui tristique, egestas turpis. Proin facilisis justo orci, vitae tristique nulla convallis eu. Cras bibendum non ante quis consectetur. Vivamus vestibulum accumsan felis, eu ornare arcu euismod semper. Aenean faucibus fringilla est, ut vulputate mi sodales id. Aenean ullamcorper enim ipsum, vitae sodales quam tincidunt condimentum. Vivamus aliquet elit sed consectetur mollis. Sed blandit lectus eget neque accumsan rutrum.

Fusce id tellus dictum, dignissim ante ac, fermentum dui. Sed eget auctor eros. Vivamus vel tristique urna. Nam ullamcorper sapien urna, vitae scelerisque eros facilisis et. Sed bibendum turpis id velit fermentum, eu mattis erat posuere. Vivamus ornare est sit amet felis condimentum condimentum. Ut id iaculis arcu. Mauris pharetra vestibulum est sit amet finibus. Sed at neque risus. Mauris nulla mauris, efficitur et iaculis et, tincidunt vitae libero. Nunc euismod nulla eget erat convallis blandit vitae id tortor. Pellentesque vitae magna a tortor scelerisque cursus laoreet nec erat. Praesent congue dui in turpis placerat, id ultricies orci varius.

Curabitur malesuada magna eu elit venenatis rhoncus. Nunc id elit eu nisi euismod dictum sit amet quis nulla. Cras hendrerit neque tellus, sed viverra ante tristique nec. Fusce sagittis porttitor purus, eu imperdiet sapien bibendum ac. Aliquam erat volutpat. Vestibulum vitae purus non dolor efficitur ullamcorper. Nunc velit mauris, accumsan eu porttitor quis, mattis eu augue. Nunc suscipit nec sapien nec feugiat. Ut elementum, ante at commodo consequat, ex enim venenatis mauris, tempus elementum lacus quam eu risus. Proin erat lorem, aliquam vitae vulputate sit amet, sagittis vitae dolor. Duis vel neque ligula. Cras semper ligula id viverra gravida. Nulla tempus nibh et tempor commodo. Sed bibendum sed quam commodo cursus. ";
*/
