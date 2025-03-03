use std::{
    io::{BufWriter, Write},
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
    time::Duration,
};

use serialport::{SerialPort, SerialPortInfo, SerialPortType};
use tracing::{debug, error, info};

use crate::app::Event;

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
    Connect(SerialPortInfo),
    TxBuffer(Vec<u8>),
    RequestReconnect,
    Disconnect,
    Shutdown(Sender<()>),
}

pub struct SerialHandle {
    command_tx: Sender<SerialCommand>,
}

impl SerialHandle {
    pub fn new(event_tx: Sender<Event>) -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = mpsc::channel();

        let mut worker = SerialWorker::new(command_rx, event_tx);

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("Serial worker encountered an error!");
        });

        let mut handle = Self { command_tx };
        handle.request_port_scan();
        (handle, worker)
    }
    pub fn connect(&mut self, port: &SerialPortInfo) {
        self.command_tx
            .send(SerialCommand::Connect(port.to_owned()))
            .unwrap();
    }
    /// Sends the supplied bytes through the connected Serial device.
    /// Newlines are automatically appended by the serial worker.
    pub fn send_bytes(&mut self, input: Vec<u8>) {
        self.command_tx
            .send(SerialCommand::TxBuffer(input))
            .unwrap();
    }
    pub fn send_str(&mut self, input: &str) {
        // debug!("Outputting to serial: {input}");
        let buffer = input.as_bytes().to_owned();
        self.send_bytes(buffer);
    }
    /// Non-blocking request for the serial worker to scan for ports and send a list of available ports
    pub fn request_port_scan(&mut self) {
        self.command_tx
            .send(SerialCommand::RequestPortScan)
            .unwrap();
    }
    /// Non-blocking request for the serial worker to attempt to reconnect to the "current" device
    pub fn request_reconnect(&mut self) {
        self.command_tx
            .send(SerialCommand::RequestReconnect)
            .unwrap();
    }
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
    connected_port_info: Option<SerialPortInfo>,
    scan_snapshot: Vec<SerialPortInfo>,
    buffer: Vec<u8>,
}

impl SerialWorker {
    fn new(command_rx: Receiver<SerialCommand>, event_tx: Sender<Event>) -> Self {
        Self {
            command_rx,
            event_tx,
            port: None,
            connected_port_info: None,
            scan_snapshot: vec![],
            buffer: vec![0; 1024 * 1024],
        }
    }
    fn work_loop(&mut self) -> color_eyre::Result<()> {
        loop {
            match self.command_rx.try_recv() {
                Ok(cmd) => match cmd {
                    // TODO: Catch failures to connect here instead of propogating to the whole task
                    SerialCommand::Connect(port) => {
                        self.connect_to_port(&port)?;
                    }
                    SerialCommand::Shutdown(shutdown_tx) => {
                        shutdown_tx
                            .send(())
                            .expect("Failed to reply to shutdown request");

                        _ = self.connected_port_info.take();
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
                        _ = self.connected_port_info.take();
                        _ = self.port.take();
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
                match port.read(self.buffer.as_mut_slice()) {
                    Ok(t) if t > 0 => {
                        let cloned_buff = self.buffer[..t].to_owned();
                        // info!("{:?}", &serial_buf[..t]);
                        self.event_tx
                            .send(SerialEvent::RxBuffer(cloned_buff).into())?;
                    }
                    // 0-size read, ignoring
                    Ok(_) => (),

                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => (),
                    Err(e) => {
                        error!("{:?}", e);
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
        assert!(self.connected_port_info.is_some());
        // assert!(self.port.is_none());
        if self.port.is_some() {
            error!("Got request to reconnect when already connected to port! Not acting...");
            return Ok(());
        }
        let current_ports = self.scan_for_serial_ports()?;
        let desired_port = self.connected_port_info.as_ref().unwrap();

        // Checking for a perfect match
        if let Some(port) = current_ports.iter().find(|p| *p == desired_port) {
            info!("Perfect match found! Reconnecting to: {}", port.port_name);
            // Sleeping to give the device some time to intialize with Windows
            // (Otherwise Access Denied errors can occur from trying to connect too quick)
            std::thread::sleep(Duration::from_secs(1));
            self.connect_to_port(&port)?;
            return Ok(());
        };

        let desired_is_usb = matches!(desired_port.port_type, SerialPortType::UsbPort(_));
        if desired_is_usb {
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
                info!("Connecting to similar device on port: {}", port.port_name);
                std::thread::sleep(Duration::from_secs(1));
                self.connect_to_port(&port)?;
                return Ok(());
            };
            // Maybe add an extra USB attempt that just tries based on *just* USB PID & VID?
            // (As some devices seem to change their Serial # arbitrarily?)
            // Could be a toggle with Strict/Loose options.
        }

        // Last ditch effort, just try to connect to the same port_name if it's present.
        if let Some(port) = current_ports
            .iter()
            .find(|p| *p.port_name == desired_port.port_name)
        {
            info!("Last ditch connect attempt on: {}", port.port_name);
            std::thread::sleep(Duration::from_secs(1));
            self.connect_to_port(&port)?;
            return Ok(());
        }

        // {}

        Ok(())
    }
    fn scan_for_serial_ports(&mut self) -> Result<Vec<SerialPortInfo>, serialport::Error> {
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
    fn connect_to_port(&mut self, port_info: &SerialPortInfo) -> Result<(), serialport::Error> {
        let port = if port_info.port_name.eq(MOCK_PORT_NAME) {
            let mut virt_port =
                virtual_serialport::VirtualPort::loopback(115200, MOCK_DATA.len() as u32)?;
            virt_port.write_all(MOCK_DATA.as_bytes())?;

            virt_port.into_boxed()
        } else {
            serialport::new(&port_info.port_name, 115200)
                // .flow_control(serialport::FlowControl::Software)
                .open()?
        };
        // let port = serialport::new(port, 115200).open()?;
        self.port = Some(port);
        self.connected_port_info = Some(port_info.to_owned());

        // Blech, if connecting from current_ports in attempt_reconnect, this may not exist.
        // self.connected_port_info = self
        //     .scanned_ports
        //     .iter()
        //     .find(|p| p.port_name == port_info)
        //     .cloned();

        assert!(self.connected_port_info.is_some());
        info!("Serial worker connected to: {}", port_info.port_name);
        self.event_tx.send(SerialEvent::Connected.into()).unwrap();
        Ok(())
    }
}

const MOCK_PORT_NAME: &str = "lorem-ipsum";

const MOCK_DATA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Duis porta volutpat magna non suscipit. Fusce rhoncus placerat metus, in posuere elit porta eget. Praesent ut nulla euismod, pulvinar tellus a, interdum ipsum. Integer in risus vulputate, finibus sem a, mattis ipsum. Aenean nec hendrerit tellus. Fusce risus dolor, sagittis non libero tristique, mattis vulputate libero. Proin ultrices luctus malesuada. Vestibulum non condimentum augue. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Vestibulum ultricies quis neque non pharetra. Nam fringilla nisl at tortor malesuada cursus. Nulla dictum, sem ac dignissim ullamcorper, est purus interdum tellus, at sagittis arcu risus suscipit neque. Mauris varius mauris vitae mi sollicitudin eleifend.

Donec feugiat, arcu sit amet ullamcorper consequat, nibh dolor laoreet risus, ut tincidunt tortor felis sed lacus. Aenean facilisis, mi nec feugiat rhoncus, dui urna malesuada erat, id mollis ipsum lectus ut ex. Curabitur semper vel tortor in finibus. Maecenas elit dui, cursus condimentum venenatis nec, cursus eget nisl. Proin consequat rhoncus tempor. Etiam dictum purus erat, sed aliquam mauris euismod vitae. Vivamus ut eros varius, posuere dolor eget, pretium tellus. Nam non lorem quis massa luctus hendrerit. Phasellus lobortis sodales quam in scelerisque. Morbi euismod et enim id dignissim. Sed commodo purus non est pellentesque euismod. Donec tincidunt dolor a ante aliquam auctor. Nam eget blandit felis.

Curabitur in tincidunt nunc. Phasellus in metus est. Nulla facilisi. Mauris dapibus augue non urna efficitur, eu ultrices est pellentesque. Nam semper vel nisi a pretium. Aenean malesuada sagittis mi, sit amet tempor mi. Donec at bibendum felis. Mauris a tortor luctus, tincidunt dui tristique, egestas turpis. Proin facilisis justo orci, vitae tristique nulla convallis eu. Cras bibendum non ante quis consectetur. Vivamus vestibulum accumsan felis, eu ornare arcu euismod semper. Aenean faucibus fringilla est, ut vulputate mi sodales id. Aenean ullamcorper enim ipsum, vitae sodales quam tincidunt condimentum. Vivamus aliquet elit sed consectetur mollis. Sed blandit lectus eget neque accumsan rutrum.

Fusce id tellus dictum, dignissim ante ac, fermentum dui. Sed eget auctor eros. Vivamus vel tristique urna. Nam ullamcorper sapien urna, vitae scelerisque eros facilisis et. Sed bibendum turpis id velit fermentum, eu mattis erat posuere. Vivamus ornare est sit amet felis condimentum condimentum. Ut id iaculis arcu. Mauris pharetra vestibulum est sit amet finibus. Sed at neque risus. Mauris nulla mauris, efficitur et iaculis et, tincidunt vitae libero. Nunc euismod nulla eget erat convallis blandit vitae id tortor. Pellentesque vitae magna a tortor scelerisque cursus laoreet nec erat. Praesent congue dui in turpis placerat, id ultricies orci varius.

Curabitur malesuada magna eu elit venenatis rhoncus. Nunc id elit eu nisi euismod dictum sit amet quis nulla. Cras hendrerit neque tellus, sed viverra ante tristique nec. Fusce sagittis porttitor purus, eu imperdiet sapien bibendum ac. Aliquam erat volutpat. Vestibulum vitae purus non dolor efficitur ullamcorper. Nunc velit mauris, accumsan eu porttitor quis, mattis eu augue. Nunc suscipit nec sapien nec feugiat. Ut elementum, ante at commodo consequat, ex enim venenatis mauris, tempus elementum lacus quam eu risus. Proin erat lorem, aliquam vitae vulputate sit amet, sagittis vitae dolor. Duis vel neque ligula. Cras semper ligula id viverra gravida. Nulla tempus nibh et tempor commodo. Sed bibendum sed quam commodo cursus. ";
