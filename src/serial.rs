use std::{
    io::Write,
    sync::mpsc::{self, Receiver, Sender},
};

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
        let mut serial_buf: Vec<u8> = vec![0; 1024];
        loop {
            match self.command_rx.try_recv() {
                Ok(cmd) => match cmd {
                    // TODO: Catch failures to connect here instead of propogating to the whole task
                    SerialCommand::Connect { port } => {
                        self.connect_to_port(&port)?;
                        info!("Connected to {port}");
                    }

                    SerialCommand::Disconnect => std::mem::drop(self.port.take()),
                    // This should maybe reply with a success/fail in case the
                    // port is having an issue, so the user's input buffer isn't consumed visually
                    // SerialCommand::TxBuffer
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => (),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }

            if let Some(port) = &mut self.port {
                // if port.bytes_to_read().unwrap() == 0 {
                //     continue;
                // }
                match port.read(serial_buf.as_mut_slice()) {
                    Ok(t) if t > 0 => {
                        let cloned_buff = serial_buf[..t].to_owned();
                        // info!("{:?}", &serial_buf[..t]);
                        self.event_tx
                            .send(Event::Serial(SerialEvent::RxBuffer(cloned_buff)))?;
                    }
                    // 0-size read, ignoring
                    Ok(_) => (),

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

        // Not actually unreachable, but I want it to crash if it gets here (for now)
        unreachable!()
    }
    pub fn connect_to_port(&mut self, port: &str) -> color_eyre::Result<()> {
        let port = if port.starts_with("virtual") {
            let mut virt_port =
                virtual_serialport::VirtualPort::loopback(115200, MOCK_DATA.len() as u32)?;
            virt_port.write_all(MOCK_DATA.as_bytes())?;

            virt_port.into_boxed()
        } else {
            serialport::new(port, 115200).open()?
        };
        // let port = serialport::new(port, 115200).open()?;
        self.port = Some(port);

        Ok(())
    }
}

const MOCK_DATA: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Duis porta volutpat magna non suscipit. Fusce rhoncus placerat metus, in posuere elit porta eget. Praesent ut nulla euismod, pulvinar tellus a, interdum ipsum. Integer in risus vulputate, finibus sem a, mattis ipsum. Aenean nec hendrerit tellus. Fusce risus dolor, sagittis non libero tristique, mattis vulputate libero. Proin ultrices luctus malesuada. Vestibulum non condimentum augue. Vestibulum ante ipsum primis in faucibus orci luctus et ultrices posuere cubilia curae; Vestibulum ultricies quis neque non pharetra. Nam fringilla nisl at tortor malesuada cursus. Nulla dictum, sem ac dignissim ullamcorper, est purus interdum tellus, at sagittis arcu risus suscipit neque. Mauris varius mauris vitae mi sollicitudin eleifend.

Donec feugiat, arcu sit amet ullamcorper consequat, nibh dolor laoreet risus, ut tincidunt tortor felis sed lacus. Aenean facilisis, mi nec feugiat rhoncus, dui urna malesuada erat, id mollis ipsum lectus ut ex. Curabitur semper vel tortor in finibus. Maecenas elit dui, cursus condimentum venenatis nec, cursus eget nisl. Proin consequat rhoncus tempor. Etiam dictum purus erat, sed aliquam mauris euismod vitae. Vivamus ut eros varius, posuere dolor eget, pretium tellus. Nam non lorem quis massa luctus hendrerit. Phasellus lobortis sodales quam in scelerisque. Morbi euismod et enim id dignissim. Sed commodo purus non est pellentesque euismod. Donec tincidunt dolor a ante aliquam auctor. Nam eget blandit felis.

Curabitur in tincidunt nunc. Phasellus in metus est. Nulla facilisi. Mauris dapibus augue non urna efficitur, eu ultrices est pellentesque. Nam semper vel nisi a pretium. Aenean malesuada sagittis mi, sit amet tempor mi. Donec at bibendum felis. Mauris a tortor luctus, tincidunt dui tristique, egestas turpis. Proin facilisis justo orci, vitae tristique nulla convallis eu. Cras bibendum non ante quis consectetur. Vivamus vestibulum accumsan felis, eu ornare arcu euismod semper. Aenean faucibus fringilla est, ut vulputate mi sodales id. Aenean ullamcorper enim ipsum, vitae sodales quam tincidunt condimentum. Vivamus aliquet elit sed consectetur mollis. Sed blandit lectus eget neque accumsan rutrum.

Fusce id tellus dictum, dignissim ante ac, fermentum dui. Sed eget auctor eros. Vivamus vel tristique urna. Nam ullamcorper sapien urna, vitae scelerisque eros facilisis et. Sed bibendum turpis id velit fermentum, eu mattis erat posuere. Vivamus ornare est sit amet felis condimentum condimentum. Ut id iaculis arcu. Mauris pharetra vestibulum est sit amet finibus. Sed at neque risus. Mauris nulla mauris, efficitur et iaculis et, tincidunt vitae libero. Nunc euismod nulla eget erat convallis blandit vitae id tortor. Pellentesque vitae magna a tortor scelerisque cursus laoreet nec erat. Praesent congue dui in turpis placerat, id ultricies orci varius.

Curabitur malesuada magna eu elit venenatis rhoncus. Nunc id elit eu nisi euismod dictum sit amet quis nulla. Cras hendrerit neque tellus, sed viverra ante tristique nec. Fusce sagittis porttitor purus, eu imperdiet sapien bibendum ac. Aliquam erat volutpat. Vestibulum vitae purus non dolor efficitur ullamcorper. Nunc velit mauris, accumsan eu porttitor quis, mattis eu augue. Nunc suscipit nec sapien nec feugiat. Ut elementum, ante at commodo consequat, ex enim venenatis mauris, tempus elementum lacus quam eu risus. Proin erat lorem, aliquam vitae vulputate sit amet, sagittis vitae dolor. Duis vel neque ligula. Cras semper ligula id viverra gravida. Nulla tempus nibh et tempor commodo. Sed bibendum sed quam commodo cursus. ";
