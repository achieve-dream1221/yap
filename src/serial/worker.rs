use std::{
    io::Write,
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
    time::{Duration, Instant},
};

use arc_swap::ArcSwap;

use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use tracing::{debug, error, info, warn};
use virtual_serialport::VirtualPort;

use crate::{
    app::{Event, Tick},
    errors::{YapError, YapResult},
    serial::SerialEvent,
    settings::PortSettings,
    traits::ToggleBool,
};

#[cfg(feature = "espflash")]
use super::esp::EspFlashEvent;
use super::{ReconnectType, Reconnections, SerialSignals, handle::SerialCommand};
#[cfg(feature = "espflash")]
use espflash::connection::reset::ResetStrategy;

#[cfg(unix)]
pub type NativePort = serialport::TTYPort;
#[cfg(windows)]
pub type NativePort = serialport::COMPort;

#[derive(Default)]
enum TakeablePort {
    #[default]
    None,
    Borrowed,
    Native(NativePort),
    Loopback(VirtualPort),
}

impl TakeablePort {
    fn is_none(&self) -> bool {
        matches!(self, TakeablePort::None)
    }
    fn is_some(&self) -> bool {
        !self.is_none()
    }
    fn is_borrowed(&self) -> bool {
        matches!(self, TakeablePort::Borrowed)
    }
    fn is_owned(&self) -> bool {
        matches!(self, TakeablePort::Native(_) | TakeablePort::Loopback(_))
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
    command_rx: Receiver<SerialCommand>,
    event_tx: Sender<Event>,
    port: TakeablePort,
    last_signal_check: Instant,
    // settings: PortSettings,
    scan_snapshot: Vec<SerialPortInfo>,
    rx_buffer: Vec<u8>,
    shared_status: Arc<ArcSwap<PortStatus>>,
    shared_settings: Arc<ArcSwap<PortSettings>>,
    // port_status: SerialStatus,
}

impl SerialWorker {
    pub fn new(
        command_rx: Receiver<SerialCommand>,
        event_tx: Sender<Event>,
        port_status: Arc<ArcSwap<PortStatus>>,
        port_settings: Arc<ArcSwap<PortSettings>>,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            shared_status: port_status,
            shared_settings: port_settings,
            // port_status: SerialStatus::idle(),
            // connected_port_info: None,
            port: TakeablePort::default(),
            last_signal_check: Instant::now(),
            // settings: PortSettings::default(),
            scan_snapshot: vec![],
            rx_buffer: vec![0; 1024 * 1024],
        }
    }
    // Primary loop for this thread.
    // Only use `?` on operations that we expect to run flawlessly, and that
    // should kill the port connection if encountered.
    pub fn work_loop(&mut self) -> color_eyre::Result<()> {
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
                Ok(cmd) => {
                    match cmd {
                        // TODO: Catch failures to connect here instead of propogating to the whole task
                        SerialCommand::Connect { port, settings } => {
                            // self.settings.baud_rate = baud_rate;
                            self.update_settings(settings)?;
                            self.connect_to_port(&port, None)?;
                        }
                        SerialCommand::PortSettings(settings) => self.update_settings(settings)?,
                        #[cfg(feature = "espflash")]
                        SerialCommand::EspRestart { bootloader } => {
                            if let Some(port) = self.port.as_mut_native_port() {
                                let strategy = super::esp::TestReset::new();
                                // let strategy = espflash::connection::reset::ClassicReset::new(false);
                                strategy.reset(port)?;
                                // let strategy = espflash::connection::reset::ClassicReset::new(true);
                                // strategy.reset(port)?;

                                // espflash::flasher::Flasher::

                                let mut status: PortStatus =
                                    self.shared_status.load().as_ref().clone();

                                let usb_port_info = {
                                    match &status.current_port {
                                        None => unreachable!(),
                                        Some(info) => match &info.port_type {
                                            SerialPortType::UsbPort(e) => e.clone(),
                                            _ => unreachable!(),
                                        },
                                    }
                                };

                                status.state = PortState::LentOut;

                                self.shared_status.store(Arc::new(status.clone()));

                                self.event_tx
                                    .send(EspFlashEvent::PortBorrowed.into())
                                    .unwrap();

                                let lent_port = self.port.take_native().unwrap();

                                let mut returned_port = if bootloader {
                                    let mut flasher = espflash::flasher::Flasher::connect(
                                        lent_port,
                                        usb_port_info,
                                        Some(115200),
                                        true,
                                        true,
                                        true,
                                        None,
                                        espflash::connection::reset::ResetAfterOperation::HardReset,
                                        espflash::connection::reset::ResetBeforeOperation::DefaultReset,
                                    )
                                    .unwrap();

                                    let esp_chip = flasher.chip();

                                    let esp_info = flasher.device_info().unwrap();

                                    debug!("{esp_info:?}");

                                    self.event_tx
                                        .send(
                                            EspFlashEvent::BootloaderSuccess {
                                                chip: esp_chip.to_string().to_uppercase(),
                                            }
                                            .into(),
                                        )
                                        .unwrap();

                                    flasher.into_serial()
                                } else {
                                    let mut connection = espflash::connection::Connection::new(
                                        lent_port, usb_port_info, espflash::connection::reset::ResetAfterOperation::HardReset,
                                        espflash::connection::reset::ResetBeforeOperation::DefaultReset);

                                    connection.reset().unwrap();

                                    self.event_tx
                                        .send(EspFlashEvent::HardResetAttempt.into())
                                        .unwrap();

                                    connection.into_serial()
                                };

                                returned_port
                                    .set_timeout(Duration::from_millis(100))
                                    .unwrap();

                                let baud_rate = self.shared_settings.load().baud_rate;
                                returned_port.set_baud_rate(baud_rate).unwrap();
                                returned_port
                                    .write_data_terminal_ready(status.signals.dtr)
                                    .unwrap();
                                returned_port
                                    .write_request_to_send(status.signals.rts)
                                    .unwrap();

                                self.port.return_native(returned_port);

                                status.state = PortState::Connected;

                                self.shared_status.store(Arc::new(status));

                                self.event_tx
                                    .send(SerialEvent::Connected(None).into())
                                    .unwrap();

                                // flasher
                                //     .write_bin_to_flash(
                                //         0,
                                //         include_bytes!("../OpenShock_Pishock-2023_1.4.0.bin"),
                                //         None,
                                //     )
                                //     .unwrap();
                            } else {
                                error!("Requested an ESP restart with no port active!");
                            }
                        }

                        // This actually does work!
                        // Just needs a helluva lot of logic and polish to work in a presentable manner
                        // SerialCommand::EspFlashing(_) => {
                        // let port_info = {
                        //     let status_guard = self.shared_port_status.load();
                        //     match &status_guard.current_port {
                        //         None => panic!(),
                        //         Some(info) => match &info.port_type {
                        //             SerialPortType::UsbPort(e) => e.clone(),
                        //             _ => unreachable!(),
                        //         },
                        //     }
                        // };
                        // let mut flasher = espflash::flasher::Flasher::connect(
                        //     self.port.take().unwrap(),
                        //     port_info,
                        //     Some(921600),
                        //     true,
                        //     true,
                        //     true,
                        //     None,
                        //     espflash::connection::reset::ResetAfterOperation::HardReset,
                        //     espflash::connection::reset::ResetBeforeOperation::DefaultReset,
                        // )
                        // .unwrap();
                        // flasher
                        //     .write_bin_to_flash(
                        //         0,
                        //         include_bytes!("../OpenShock_Pishock-2023_1.4.0.bin"),
                        //         None,
                        //     )
                        //     .unwrap();
                        //     let mut port = flasher.into_serial();
                        //     port.set_timeout(Duration::from_millis(100))?;
                        //     port.clear(serialport::ClearBuffer::All)?;
                        //     port.flush()?;
                        //     while let Ok(_) = self.command_rx.try_recv() {
                        //         // draining messages that piled up during send
                        //         // might need to instead move the flashing to a different thread..?
                        //         // and return the port afterwards?
                        //         // might work since i can send progress updates here?
                        //         // but not sure why i wouldnt just send to main/ui thread
                        //     }
                        //     port.set_baud_rate(self.baud_rate)?;
                        //     self.port = Some(port);
                        //     info!("Port ownership returned to terminal!");
                        // },
                        // SerialCommand::ReadSignals => self.read_and_share_serial_signals(false)?,
                        SerialCommand::WriteSignals { dtr, rts } => {
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
                        SerialCommand::ToggleSignals { dtr, rts } => {
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
                        SerialCommand::Shutdown(shutdown_tx) => {
                            shutdown_tx
                                .send(())
                                .expect("Failed to reply to shutdown request");

                            self.shared_status
                                .store(Arc::new(PortStatus::new_idle(&PortSettings::default())));
                            self.port.drop();
                            break;
                        }
                        SerialCommand::RequestPortScan => {
                            let ports = self.scan_for_serial_ports().unwrap();
                            self.scan_snapshot = ports.clone();
                            self.event_tx
                                .send(SerialEvent::Ports(ports).into())
                                .unwrap();
                        }
                        SerialCommand::RequestReconnect => {
                            self.attempt_reconnect().unwrap();
                        }
                        SerialCommand::Disconnect => {
                            // self.port_status = SerialStatus::idle();

                            let settings = self.shared_settings.load();

                            let previous_status = { self.shared_status.load().as_ref().clone() };

                            self.shared_status
                                .store(Arc::new(previous_status.to_idle(&*settings)));
                            self.port.drop();
                            self.event_tx.send(SerialEvent::Disconnected(None).into())?;
                        }
                        // This should maybe reply with a success/fail in case the
                        // port is having an issue, so the user's input buffer isn't consumed visually
                        SerialCommand::TxBuffer(mut data) if self.port.is_owned() => {
                            let port = self.port.as_mut_port().unwrap();
                            info!(
                                "bytes incoming: {}, bytes outcoming: {}",
                                port.bytes_to_read().unwrap(),
                                port.bytes_to_write().unwrap()
                            );

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
                                        todo!();
                                    }
                                    Ok(n) => {
                                        // info!(
                                        //     "bytes incoming: {}, bytes outcoming: {}",
                                        //     port.bytes_to_read().unwrap(),
                                        //     port.bytes_to_write().unwrap()
                                        // );
                                        info!("buf n: {n}");
                                        buf = &buf[n..];
                                        self.event_tx.send(Tick::Tx.into()).unwrap();
                                        std::thread::sleep(Duration::from_millis(1));
                                    }
                                    Err(e) => todo!("{e}"),
                                }
                            }

                            // if let Err(e) = port.write_all(&data) {
                            //     todo!("{e}");
                            // } else {
                            //     info!("{data:?}");
                            //     port.flush()?;
                            // }
                        }
                        SerialCommand::TxBuffer(_) => todo!("Tried to send with no port"), // Tried to send with no port
                    }
                }

                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // info!("no message");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("Worker lost all Handles");
                }
            }

            if let Some(port) = self.port.as_mut_port() {
                // if port.bytes_to_read().unwrap() == 0 {
                //     continue;
                // }
                // info!(
                //     "bytes incoming: {}, bytes outcoming: {}",
                //     port.bytes_to_read().unwrap(),
                //     port.bytes_to_write().unwrap()
                // );
                match port.read(self.rx_buffer.as_mut_slice()) {
                    Ok(t) if t > 0 => {
                        let cloned_buff = self.rx_buffer[..t].to_owned();
                        // info!("{:?}", &serial_buf[..t]);
                        self.event_tx
                            .send(SerialEvent::RxBuffer(cloned_buff).into())?;
                    }
                    // 0-size read, ignoring
                    Ok(_) => (),

                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => (),
                    Err(e) => {
                        error!("{:?}", e);
                        // TODO might need to reconsider this
                        // since this assumes that on error, the port is *gone*
                        // which *is* possible, but not a guarantee
                        // (maybe i could manually pop it if it still exists? since i do have connected_port_info as a reference)
                        // TODO Also reconsider error handling, should it take down the task? Or allow failures?
                        self.scan_snapshot = self.scan_for_serial_ports()?;

                        self.port.drop();
                        let disconnected_status = {
                            self.shared_status
                                .load()
                                .as_ref()
                                .clone()
                                // Ensure we keep around the old SerialPortInfo to use
                                // as a reference for reconnections!
                                .to_unhealthy()
                        };
                        self.shared_status.store(Arc::new(disconnected_status));

                        self.event_tx
                            .send(SerialEvent::Disconnected(Some(e.to_string())).into())?;
                    }
                }

                if self.last_signal_check.elapsed() >= Duration::from_millis(100) {
                    self.read_and_share_serial_signals(false)?;
                    self.last_signal_check = Instant::now();
                }
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
    fn attempt_reconnect(&mut self) -> Result<(), serialport::Error> {
        // assert!(self.connected_port_info.read().unwrap().is_some());
        // assert!(self.port.is_none());
        if self.port.is_owned() || self.port.is_borrowed() {
            error!("Got request to reconnect when already connected to port! Not acting...");
            return Ok(());
        }

        let reconnections = { self.shared_settings.load().reconnections.clone() };

        if reconnections == Reconnections::Disabled {
            error!("Got request to reconnect when reconnections are disabled!");
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
            SerialPortType::UsbPort(usb) => {
                // Hardcoded filter for Index/Beyond's Bluetooth COM Port
                // Will want to make this a proper configurable filter soon.
                if usb.vid == 0x28DE && usb.pid == 0x2102 {
                    false
                } else {
                    true
                }
            }
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
    ) -> Result<(), serialport::Error> {
        let mut port_status: PortStatus = self.shared_status.load().as_ref().clone();
        // If this is a normal connection, then this should be set to settings.dtr_on_open
        // otherwise, if we're reconnecting, then this should match the state of DTR at the time of disconnection
        let dtr_on_open = port_status.signals.dtr;
        let settings = self.shared_settings.load();
        let baud_rate = settings.baud_rate;

        if port_info.port_name.eq(MOCK_PORT_NAME) {
            let mut virt_port =
                virtual_serialport::VirtualPort::loopback(baud_rate, MOCK_DATA.len() as u32)?;
            virt_port.write_all(MOCK_DATA.as_bytes())?;

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

        let port = self.port.as_mut_port().unwrap();

        port.write_request_to_send(port_status.signals.rts)?;

        // let port = serialport::new(port, 115200).open()?;
        // let port_status = SerialStatus::connected(
        //     port_info.to_owned(),
        //     baud_rate,
        //     SerialSignals::new_from_port(port.as_mut())?,
        // );
        port_status.signals.update_with_port(port)?;
        port_status.current_port = Some(port_info.to_owned());
        port_status.state = PortState::Connected;
        self.shared_status.store(Arc::new(port_status));

        // Blech, if connecting from current_ports in attempt_reconnect, this may not exist.
        // self.connected_port_info = self
        //     .scanned_ports
        //     .iter()
        //     .find(|p| p.port_name == port_info)
        //     .cloned();

        // assert!(self.connected_port_info.is_some());
        info!(
            "Serial worker connected to: {} @ {baud_rate} baud",
            port_info.port_name
        );
        // info!("port.baud_rate {}", self.port.as_ref().unwrap().baud_rate()?);
        self.event_tx
            .send(SerialEvent::Connected(reconnect_type).into())
            .unwrap();
        Ok(())
    }
    fn read_and_share_serial_signals(
        &mut self,
        force_share: bool,
    ) -> Result<(), serialport::Error> {
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
                        .send(Tick::Requested("Serial Signals").into())
                        .unwrap();
                }
            }
        }

        Ok(())
    }
}

// This status struct leaves a bit to be desired
// especially in terms of the signals and their initial states
// (between connections and app start)
// maybe something better will come to me.

#[derive(Debug, Clone, Copy, Default)]
pub enum PortState {
    #[default]
    Idle,
    PrematureDisconnect,
    LentOut,
    Connected,
}

impl PortState {
    pub fn is_healthy(&self) -> bool {
        matches!(self, PortState::Connected)
    }
    pub fn is_lent_out(&self) -> bool {
        matches!(self, PortState::LentOut)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PortStatus {
    pub state: PortState,
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
            state: PortState::PrematureDisconnect,
            ..self
        }
    }
    /// Used when the user chooses to disconnect from the serial port
    fn to_idle(self, settings: &PortSettings) -> Self {
        Self {
            state: PortState::Idle,
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

const MOCK_DATA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Duis porta volutpat magna non suscipit. Fusce rhoncus placerat metus, in posuere elit porta eget. Praesent ut nulla euismod, pulvinar tellus a, interdum ipsum. Integer in risus vulputate, finibus sem a, mattis ipsum. Aenean nec hendrerit tellus. Fusce risus dolor, sagittis non libero tristique, mattis vulputate libero. Proin ultrices luctus malesuada. Vestibulum non condimentum augue. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Vestibulum ultricies quis neque non pharetra. Nam fringilla nisl at tortor malesuada cursus. Nulla dictum, sem ac dignissim ullamcorper, est purus interdum tellus, at sagittis arcu risus suscipit neque. Mauris varius mauris vitae mi sollicitudin eleifend.

Donec feugiat, arcu sit amet ullamcorper consequat, nibh dolor laoreet risus, ut tincidunt tortor felis sed lacus. Aenean facilisis, mi nec feugiat rhoncus, dui urna malesuada erat, id mollis ipsum lectus ut ex. Curabitur semper vel tortor in finibus. Maecenas elit dui, cursus condimentum venenatis nec, cursus eget nisl. Proin consequat rhoncus tempor. Etiam dictum purus erat, sed aliquam mauris euismod vitae. Vivamus ut eros varius, posuere dolor eget, pretium tellus. Nam non lorem quis massa luctus hendrerit. Phasellus lobortis sodales quam in scelerisque. Morbi euismod et enim id dignissim. Sed commodo purus non est pellentesque euismod. Donec tincidunt dolor a ante aliquam auctor. Nam eget blandit felis.

Curabitur in tincidunt nunc. Phasellus in metus est. Nulla facilisi. Mauris dapibus augue non urna efficitur, eu ultrices est pellentesque. Nam semper vel nisi a pretium. Aenean malesuada sagittis mi, sit amet tempor mi. Donec at bibendum felis. Mauris a tortor luctus, tincidunt dui tristique, egestas turpis. Proin facilisis justo orci, vitae tristique nulla convallis eu. Cras bibendum non ante quis consectetur. Vivamus vestibulum accumsan felis, eu ornare arcu euismod semper. Aenean faucibus fringilla est, ut vulputate mi sodales id. Aenean ullamcorper enim ipsum, vitae sodales quam tincidunt condimentum. Vivamus aliquet elit sed consectetur mollis. Sed blandit lectus eget neque accumsan rutrum.

Fusce id tellus dictum, dignissim ante ac, fermentum dui. Sed eget auctor eros. Vivamus vel tristique urna. Nam ullamcorper sapien urna, vitae scelerisque eros facilisis et. Sed bibendum turpis id velit fermentum, eu mattis erat posuere. Vivamus ornare est sit amet felis condimentum condimentum. Ut id iaculis arcu. Mauris pharetra vestibulum est sit amet finibus. Sed at neque risus. Mauris nulla mauris, efficitur et iaculis et, tincidunt vitae libero. Nunc euismod nulla eget erat convallis blandit vitae id tortor. Pellentesque vitae magna a tortor scelerisque cursus laoreet nec erat. Praesent congue dui in turpis placerat, id ultricies orci varius.

Curabitur malesuada magna eu elit venenatis rhoncus. Nunc id elit eu nisi euismod dictum sit amet quis nulla. Cras hendrerit neque tellus, sed viverra ante tristique nec. Fusce sagittis porttitor purus, eu imperdiet sapien bibendum ac. Aliquam erat volutpat. Vestibulum vitae purus non dolor efficitur ullamcorper. Nunc velit mauris, accumsan eu porttitor quis, mattis eu augue. Nunc suscipit nec sapien nec feugiat. Ut elementum, ante at commodo consequat, ex enim venenatis mauris, tempus elementum lacus quam eu risus. Proin erat lorem, aliquam vitae vulputate sit amet, sagittis vitae dolor. Duis vel neque ligula. Cras semper ligula id viverra gravida. Nulla tempus nibh et tempor commodo. Sed bibendum sed quam commodo cursus. ";
