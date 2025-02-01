use std::sync::mpsc::{self, Receiver, Sender};

use serialport::SerialPort;
use tracing::{error, info};

use crate::app::Event;

pub enum SerialEvent {
    Connected,
    RxBuffer(Vec<u8>),
    Disconnected,
}

pub enum SerialCommand {
    Connect { port: String },
    Disconnect,
}

pub struct SerialHandle {
    command_tx: Sender<SerialCommand>,
}

impl SerialHandle {
    pub fn new(event_tx: Sender<Event>) -> Self {
        let (command_tx, command_rx) = mpsc::channel();

        let mut worker = SerialWorker::new(command_rx, event_tx);

        std::thread::spawn(move || {
            worker.work_loop().unwrap();
        });

        Self { command_tx }
    }
    pub fn connect(&mut self, port: &str) {
        self.command_tx
            .send(SerialCommand::Connect {
                port: port.to_owned(),
            })
            .unwrap();
    }
}

pub struct SerialWorker {
    command_rx: Receiver<SerialCommand>,
    event_tx: Sender<Event>,
    port: Option<Box<dyn SerialPort>>,
}

impl SerialWorker {
    fn new(command_rx: Receiver<SerialCommand>, event_tx: Sender<Event>) -> Self {
        Self {
            command_rx,
            event_tx,
            port: None,
        }
    }
    fn work_loop(&mut self) -> color_eyre::Result<()> {
        loop {
            match self.command_rx.try_recv() {
                Ok(cmd) => match cmd {
                    // TODO: Catch failures to connect here instead of propogating to the whole task
                    SerialCommand::Connect { port } => {
                        self.connect_to_port(&port)?;
                        info!("Connected to {port}");
                    }

                    SerialCommand::Disconnect => std::mem::drop(self.port.take()),
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }

            if let Some(port) = &mut self.port {
                // if port.bytes_to_read().unwrap() == 0 {
                //     continue;
                // }
                let mut serial_buf: Vec<u8> = vec![0; 1000];
                match port.read(serial_buf.as_mut_slice()) {
                    Ok(t) => {
                        serial_buf.truncate(t);
                        // info!("{:?}", &serial_buf[..t]);
                        self.event_tx
                            .send(Event::Serial(SerialEvent::RxBuffer(serial_buf)))?;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => (),
                    Err(e) => {
                        error!("{:?}", e);
                        _ = self.port.take();
                        self.event_tx
                            .send(Event::Serial(SerialEvent::Disconnected))?;
                    }
                }
            }
        }

        unreachable!()
    }
    pub fn connect_to_port(&mut self, port: &str) -> color_eyre::Result<()> {
        let port = serialport::new(port, 115200).open()?;
        self.port = Some(port);

        Ok(())
    }
}
