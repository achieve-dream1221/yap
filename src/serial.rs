use std::{
    io::{BufWriter, Write},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, RwLock,
    },
    thread::JoinHandle,
    time::Duration,
};

use arc_swap::{ArcSwap, ArcSwapOption};
use color_eyre::owo_colors::OwoColorize;
use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use tracing::{debug, error, info};

use crate::app::{Event, Tick};

// TODO maybe relegate this to the serial worker thread in case it blocks?

#[derive(Clone, Debug)]
pub enum SerialEvent {
    Ports(Vec<SerialPortInfo>),
    Connected,
    RxBuffer(Vec<u8>),
    Disconnected,
}

impl From<SerialEvent> for Event {
    fn from(value: SerialEvent) -> Self {
        Self::Serial(value)
    }
}

pub enum SerialCommand {
    RequestPortScan,
    Connect {
        port: SerialPortInfo,
        baud_rate: u32,
    },
    ChangeBaud(u32),
    TxBuffer(Vec<u8>),
    WriteSignals {
        dtr: Option<bool>,
        rts: Option<bool>,
    },
    ToggleSignals {
        dtr: bool,
        rts: bool,
    },
    ReadSignals,
    RequestReconnect,
    Disconnect,
    Shutdown(Sender<()>),
}

// This status struct leaves a bit to be desired
// especially in terms of the signals and their initial states
// (between connections and app start)
// maybe something better will come to me.

#[derive(Debug, Clone, Default)]
pub struct SerialStatus {
    pub healthy: bool,
    pub current_port: Option<SerialPortInfo>,
    pub baud_rate: u32,
    pub signals: SerialSignals,
}

impl SerialStatus {
    fn idle() -> Self {
        Self::default()
    }
    fn connected(port: SerialPortInfo, baud_rate: u32, signals: SerialSignals) -> Self {
        Self {
            healthy: true,
            current_port: Some(port),
            baud_rate,
            signals,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SerialSignals {
    // Host-controlled
    pub rts: bool,
    pub dtr: bool,
    // Slave-controlled
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

fn f() {
    let port = serialport::new("1", 1).open().unwrap();
    // port.read_
}

#[derive(Clone)]
pub struct SerialHandle {
    command_tx: Sender<SerialCommand>,
    pub port_status: Arc<ArcSwap<SerialStatus>>,
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

impl SerialHandle {
    pub fn new(event_tx: Sender<Event>) -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = mpsc::channel();

        let port_status = Arc::new(ArcSwap::from_pointee(SerialStatus::idle()));

        let mut worker = SerialWorker::new(command_rx, event_tx, port_status.clone());

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("Serial worker encountered an error!");
        });

        let mut handle = Self {
            command_tx,
            port_status,
        };
        handle.request_port_scan();
        (handle, worker)
    }
    pub fn connect(&self, port: &SerialPortInfo, baud_rate: u32) {
        self.command_tx
            .send(SerialCommand::Connect {
                port: port.to_owned(),
                baud_rate,
            })
            .unwrap();
    }
    pub fn disconnect(&self) {
        self.command_tx.send(SerialCommand::Disconnect).unwrap();
    }
    /// Sends the supplied bytes through the connected Serial device.
    /// Newlines are automatically appended by the serial worker.
    pub fn send_bytes(&self, input: Vec<u8>) {
        self.command_tx
            .send(SerialCommand::TxBuffer(input))
            .unwrap();
    }
    pub fn send_str(&self, input: &str) {
        // debug!("Outputting to serial: {input}");
        let buffer = input.as_bytes().to_owned();
        self.send_bytes(buffer);
    }
    pub fn read_signals(&self) {
        self.command_tx.send(SerialCommand::ReadSignals).unwrap();
    }
    pub fn toggle_signals(&self, dtr: bool, rts: bool) {
        self.command_tx
            .send(SerialCommand::ToggleSignals { dtr, rts })
            .unwrap();
    }
    /// Non-blocking request for the serial worker to scan for ports and send a list of available ports
    pub fn request_port_scan(&self) {
        self.command_tx
            .send(SerialCommand::RequestPortScan)
            .unwrap();
    }
    /// Non-blocking request for the serial worker to attempt to reconnect to the "current" device
    pub fn request_reconnect(&self) {
        self.command_tx
            .send(SerialCommand::RequestReconnect)
            .unwrap();
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

pub struct SerialWorker {
    command_rx: Receiver<SerialCommand>,
    event_tx: Sender<Event>,
    port: Option<Box<dyn SerialPort>>,
    baud_rate: u32,
    scan_snapshot: Vec<SerialPortInfo>,
    rx_buffer: Vec<u8>,
    shared_port_status: Arc<ArcSwap<SerialStatus>>,
    // port_status: SerialStatus,
}

impl SerialWorker {
    fn new(
        command_rx: Receiver<SerialCommand>,
        event_tx: Sender<Event>,
        port_status: Arc<ArcSwap<SerialStatus>>,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            shared_port_status: port_status,
            // port_status: SerialStatus::idle(),
            // connected_port_info: None,
            port: None,
            baud_rate: 0,
            scan_snapshot: vec![],
            rx_buffer: vec![0; 1024 * 1024],
        }
    }
    fn work_loop(&mut self) -> color_eyre::Result<()> {
        loop {
            // TODO consider sleeping here for a moment with a read_timeout?
            // or have some kind of cooldown after a 0-size serial read
            // (maybe use port.bytes_to_read() ?)
            // not sure if the barrage is what's causing weird unix issues with the ESP32-S3, need to test further
            match self.command_rx.try_recv() {
                Ok(cmd) => match cmd {
                    // TODO: Catch failures to connect here instead of propogating to the whole task
                    SerialCommand::Connect { port, baud_rate } => {
                        self.baud_rate = baud_rate;
                        self.connect_to_port(&port, baud_rate)?;
                    }
                    SerialCommand::ChangeBaud(baud_rate) => {
                        assert!(self.port.is_some());
                        let port = self.port.as_mut().unwrap();
                        port.set_baud_rate(baud_rate)?;
                        self.baud_rate = baud_rate;
                    }
                    SerialCommand::ReadSignals => self.read_and_share_serial_signals(false)?,
                    SerialCommand::WriteSignals { dtr, rts } => {
                        assert!(dtr.is_some() || rts.is_some());
                        todo!()
                    }
                    SerialCommand::ToggleSignals { dtr, rts } => {
                        assert!(dtr || rts);

                        let mut status: SerialStatus =
                            self.shared_port_status.load().as_ref().clone();

                        if dtr {
                            status.signals.dtr = !status.signals.dtr;
                        }
                        if rts {
                            status.signals.rts = !status.signals.rts;
                        }
                        if let Some(port) = &mut self.port {
                            // Sending both signals regardless of which one changed
                            // to keep them in line with the expected state in the struct.
                            port.write_data_terminal_ready(status.signals.dtr)?;
                            port.write_request_to_send(status.signals.rts)?;
                        }
                        self.shared_port_status.store(Arc::new(status));
                        self.read_and_share_serial_signals(true)?;
                    }
                    SerialCommand::Shutdown(shutdown_tx) => {
                        shutdown_tx
                            .send(())
                            .expect("Failed to reply to shutdown request");

                        self.shared_port_status
                            .store(Arc::new(SerialStatus::idle()));
                        _ = self.port.take();
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
                        self.shared_port_status
                            .store(Arc::new(SerialStatus::idle()));
                        _ = self.port.take();
                        self.event_tx.send(SerialEvent::Disconnected.into())?;
                    }
                    // This should maybe reply with a success/fail in case the
                    // port is having an issue, so the user's input buffer isn't consumed visually
                    SerialCommand::TxBuffer(mut data) if self.port.is_some() => {
                        let port = self.port.as_mut().unwrap();
                        info!(
                            "bytes incoming: {}, bytes outcoming: {}",
                            port.bytes_to_read().unwrap(),
                            port.bytes_to_write().unwrap()
                        );

                        // TODO use user-specified line-ending
                        data.push(b'\n');

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
                    SerialCommand::TxBuffer(_) => todo!(), // Tried to send with no port
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("Worker lost all Handles");
                }
            }

            if let Some(port) = &mut self.port {
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
                        self.scan_snapshot = self.scan_for_serial_ports().unwrap();
                        _ = self.port.take();
                        // Don't take since we're using it to find the port for reconnections.
                        // _ = self.connected_port_info.take();
                        self.event_tx.send(SerialEvent::Disconnected.into())?;
                    }
                }
            }
        }

        Ok(())
    }
    fn attempt_reconnect(&mut self) -> Result<(), serialport::Error> {
        // assert!(self.connected_port_info.read().unwrap().is_some());
        // assert!(self.port.is_none());
        if self.port.is_some() {
            error!("Got request to reconnect when already connected to port! Not acting...");
            return Ok(());
        }
        let current_ports = self.scan_for_serial_ports()?;
        let port_guard = self.shared_port_status.load();
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
            self.connect_to_port(&port, self.baud_rate)?;
            return Ok(());
        };

        if let SerialPortType::UsbPort(desired_usb) = &desired_port.port_type {
            // Try to find a *new* port that has all the same USB characteristics
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
                self.connect_to_port(&port, self.baud_rate)?;
                return Ok(());
            };
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
                self.connect_to_port(&port, self.baud_rate)?;
                return Ok(());
            };
        }

        // Last ditch effort, just try to connect to the same port_name if it's present.
        if let Some(port) = current_ports
            .iter()
            .find(|p| *p.port_name == desired_port.port_name)
        {
            info!("Last ditch connect attempt on: {}", port.port_name);
            std::thread::sleep(Duration::from_secs(1));
            self.connect_to_port(&port, self.baud_rate)?;
            return Ok(());
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

        info!("Serial port scanning found {} ports", ports.len());
        // self.scanned_ports = ports;
        Ok(ports)
    }
    fn connect_to_port(
        &mut self,
        port_info: &SerialPortInfo,
        baud_rate: u32,
    ) -> Result<(), serialport::Error> {
        let mut port_status: SerialStatus = self.shared_port_status.load().as_ref().clone();
        // TODO Make this a config option since this seems to have different behavior for each device.
        let dtr_on_open = port_status.signals.dtr;

        let mut port = if port_info.port_name.eq(MOCK_PORT_NAME) {
            let mut virt_port =
                virtual_serialport::VirtualPort::loopback(baud_rate, MOCK_DATA.len() as u32)?;
            virt_port.write_all(MOCK_DATA.as_bytes())?;

            virt_port.into_boxed()
        } else {
            serialport::new(&port_info.port_name, baud_rate)
                .dtr_on_open(dtr_on_open)
                // .flow_control(serialport::FlowControl::Software)
                .open()?
        };

        port.write_request_to_send(port_status.signals.rts)?;

        // let port = serialport::new(port, 115200).open()?;
        // let port_status = SerialStatus::connected(
        //     port_info.to_owned(),
        //     baud_rate,
        //     SerialSignals::new_from_port(port.as_mut())?,
        // );
        port_status.baud_rate = baud_rate;
        port_status.current_port = Some(port_info.to_owned());
        port_status.signals.update_with_port(port.as_mut())?;
        port_status.healthy = true;
        self.port = Some(port);
        self.shared_port_status.store(Arc::new(port_status));

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
        self.event_tx.send(SerialEvent::Connected.into()).unwrap();
        Ok(())
    }
    fn read_and_share_serial_signals(
        &mut self,
        force_share: bool,
    ) -> Result<(), serialport::Error> {
        let mut port_status: SerialStatus = self.shared_port_status.load().as_ref().clone();

        match &mut self.port {
            // If no port is present, just skip.
            None => return Ok(()),
            Some(port) => {
                // debug_assert later?
                assert!(self.baud_rate == port_status.baud_rate);

                let changed = port_status.signals.update_with_port(port.as_mut())?;
                // Only update the shared status if there's actually a change
                if changed {
                    self.shared_port_status.store(Arc::new(port_status));
                }
                // But always send the UI update tick if a DTR/RTS change requested it
                if changed || force_share {
                    self.event_tx.send(Tick::Requested.into()).unwrap();
                }
            }
        }

        Ok(())
    }
}

pub const MOCK_PORT_NAME: &str = "lorem-ipsum";

const MOCK_DATA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Duis porta volutpat magna non suscipit. Fusce rhoncus placerat metus, in posuere elit porta eget. Praesent ut nulla euismod, pulvinar tellus a, interdum ipsum. Integer in risus vulputate, finibus sem a, mattis ipsum. Aenean nec hendrerit tellus. Fusce risus dolor, sagittis non libero tristique, mattis vulputate libero. Proin ultrices luctus malesuada. Vestibulum non condimentum augue. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Vestibulum ultricies quis neque non pharetra. Nam fringilla nisl at tortor malesuada cursus. Nulla dictum, sem ac dignissim ullamcorper, est purus interdum tellus, at sagittis arcu risus suscipit neque. Mauris varius mauris vitae mi sollicitudin eleifend.

Donec feugiat, arcu sit amet ullamcorper consequat, nibh dolor laoreet risus, ut tincidunt tortor felis sed lacus. Aenean facilisis, mi nec feugiat rhoncus, dui urna malesuada erat, id mollis ipsum lectus ut ex. Curabitur semper vel tortor in finibus. Maecenas elit dui, cursus condimentum venenatis nec, cursus eget nisl. Proin consequat rhoncus tempor. Etiam dictum purus erat, sed aliquam mauris euismod vitae. Vivamus ut eros varius, posuere dolor eget, pretium tellus. Nam non lorem quis massa luctus hendrerit. Phasellus lobortis sodales quam in scelerisque. Morbi euismod et enim id dignissim. Sed commodo purus non est pellentesque euismod. Donec tincidunt dolor a ante aliquam auctor. Nam eget blandit felis.

Curabitur in tincidunt nunc. Phasellus in metus est. Nulla facilisi. Mauris dapibus augue non urna efficitur, eu ultrices est pellentesque. Nam semper vel nisi a pretium. Aenean malesuada sagittis mi, sit amet tempor mi. Donec at bibendum felis. Mauris a tortor luctus, tincidunt dui tristique, egestas turpis. Proin facilisis justo orci, vitae tristique nulla convallis eu. Cras bibendum non ante quis consectetur. Vivamus vestibulum accumsan felis, eu ornare arcu euismod semper. Aenean faucibus fringilla est, ut vulputate mi sodales id. Aenean ullamcorper enim ipsum, vitae sodales quam tincidunt condimentum. Vivamus aliquet elit sed consectetur mollis. Sed blandit lectus eget neque accumsan rutrum.

Fusce id tellus dictum, dignissim ante ac, fermentum dui. Sed eget auctor eros. Vivamus vel tristique urna. Nam ullamcorper sapien urna, vitae scelerisque eros facilisis et. Sed bibendum turpis id velit fermentum, eu mattis erat posuere. Vivamus ornare est sit amet felis condimentum condimentum. Ut id iaculis arcu. Mauris pharetra vestibulum est sit amet finibus. Sed at neque risus. Mauris nulla mauris, efficitur et iaculis et, tincidunt vitae libero. Nunc euismod nulla eget erat convallis blandit vitae id tortor. Pellentesque vitae magna a tortor scelerisque cursus laoreet nec erat. Praesent congue dui in turpis placerat, id ultricies orci varius.

Curabitur malesuada magna eu elit venenatis rhoncus. Nunc id elit eu nisi euismod dictum sit amet quis nulla. Cras hendrerit neque tellus, sed viverra ante tristique nec. Fusce sagittis porttitor purus, eu imperdiet sapien bibendum ac. Aliquam erat volutpat. Vestibulum vitae purus non dolor efficitur ullamcorper. Nunc velit mauris, accumsan eu porttitor quis, mattis eu augue. Nunc suscipit nec sapien nec feugiat. Ut elementum, ante at commodo consequat, ex enim venenatis mauris, tempus elementum lacus quam eu risus. Proin erat lorem, aliquam vitae vulputate sit amet, sagittis vitae dolor. Duis vel neque ligula. Cras semper ligula id viverra gravida. Nulla tempus nibh et tempor commodo. Sed bibendum sed quam commodo cursus. ";
