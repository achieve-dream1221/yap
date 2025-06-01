use std::{
    borrow::Cow,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

use bstr::ByteSlice;
use chrono::{DateTime, Local};
use compact_str::CompactString;
use fs_err as fs;
use ratatui::text::Line;
use serialport::SerialPortInfo;
use tracing::{error, warn};

use crate::{
    buffer::LineType,
    errors::{YapError, YapResult},
    serial::ReconnectType,
    settings::{self, Logging, LoggingType},
    traits::ByteSuffixCheck,
};

use super::{LineEnding, buf_line::BufLine, line_ending_iter};

pub struct LoggingHandle {
    command_tx: Sender<LoggingCommand>,
    session_open: Arc<AtomicBool>,
}

const FILE_LINE_TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S%.9f";

pub enum LoggingCommand {
    PortConnected(DateTime<Local>, SerialPortInfo, Option<ReconnectType>),
    PortDisconnect {
        timestamp: DateTime<Local>,
        intentional: bool,
    },
    RequestStart(SerialPortInfo),
    RequestStop,
    RequestClearFiles,
    RxBytes(DateTime<Local>, Vec<u8>),
    TxBytes {
        timestamp: DateTime<Local>,
        bytes: Vec<u8>,
        line_ending: Vec<u8>,
    },
    // RxLine(Line<'static>),
    // Tx(Vec<u8>, Line<'static>),
    // RxTxLine(BufLine),

    // TODO oh god handle sending the raw buffer again if we need to redo the lines?
    LineEndingChange(Vec<u8>, LineEnding),
    Settings(Logging),
    Shutdown(Sender<()>),
}

enum LoggingLineType {
    RxLine,
    TxLine { line_ending: Vec<u8> },
}

#[derive(Debug)]
struct FileAndResetIndex {
    inner: fs::File,
    // When a user performs an action that would require re-doing the log file,
    // (such as changing line-endings or hiding user input)
    // but we don't have access to the old data anymore (due to a previous intentional disconnect clearing the buffers),
    // we need to know where to safely reset the files to.
    reset_index: u64,
}

impl From<fs::File> for FileAndResetIndex {
    fn from(value: fs::File) -> Self {
        Self {
            inner: value,
            reset_index: 0,
        }
    }
}

struct LoggingWorker {
    command_rx: Receiver<LoggingCommand>,
    // path: PathBuf,
    text_file: Option<FileAndResetIndex>,
    raw_file: Option<FileAndResetIndex>,
    started_at: Option<DateTime<Local>>,
    settings: Logging,
    session_open: Arc<AtomicBool>,
    line_ending: LineEnding,
    last_rx_completed: bool,
}

impl LoggingHandle {
    pub(super) fn new(line_ending: LineEnding, settings: Logging) -> (Self, JoinHandle<()>) {
        let (command_tx, command_rx) = mpsc::channel();

        let session_open = Arc::new(AtomicBool::new(false));

        let mut worker = LoggingWorker {
            // event_tx,
            command_rx,
            settings,
            line_ending,
            // path: "meow.log".into(),
            text_file: None,
            raw_file: None,
            started_at: None,
            session_open: session_open.clone(),
            last_rx_completed: true,
        };

        let worker = std::thread::spawn(move || {
            worker
                .work_loop()
                .expect("Carousel encountered an unexpected fatal error");
        });

        (
            Self {
                command_tx,
                session_open,
            },
            worker,
        )
    }
    pub fn is_ready(&self) -> bool {
        self.session_open.load(Ordering::Acquire)
    }
    pub fn log_port_connected(
        &self,
        port_info: SerialPortInfo,
        reconnect_type: Option<ReconnectType>,
    ) -> YapResult<()> {
        self.command_tx
            .send(LoggingCommand::PortConnected(
                Local::now(),
                port_info,
                reconnect_type,
            ))
            .map_err(|_| YapError::NoLoggingWorker)?;
        Ok(())
    }
    pub(super) fn log_rx_bytes(&self, timestamp: DateTime<Local>, bytes: Vec<u8>) -> YapResult<()> {
        self.command_tx
            .send(LoggingCommand::RxBytes(timestamp, bytes))
            .map_err(|_| YapError::NoLoggingWorker)?;
        Ok(())
    }
    pub(super) fn log_tx_bytes(
        &self,
        timestamp: DateTime<Local>,
        bytes: Vec<u8>,
        line_ending: Vec<u8>,
    ) -> YapResult<()> {
        self.command_tx
            .send(LoggingCommand::TxBytes {
                timestamp,
                bytes,
                line_ending,
            })
            .map_err(|_| YapError::NoLoggingWorker)?;
        Ok(())
    }
    // pub(super) fn log_bufline(&self, line: BufLine) -> YapResult<()> {
    //     self.command_tx
    //         .send(LoggingCommand::RxTxLine(line))
    //         .map_err(|_| YapError::NoLoggingWorker)?;
    //     Ok(())
    // }
    pub(super) fn clear_current_logs(&self) -> YapResult<()> {
        self.command_tx
            .send(LoggingCommand::RequestClearFiles)
            .map_err(|_| YapError::NoLoggingWorker)?;
        Ok(())
    }
    pub fn log_port_disconnected(&self, intentional: bool) -> YapResult<()> {
        self.command_tx
            .send(LoggingCommand::PortDisconnect {
                timestamp: Local::now(),
                intentional,
            })
            .map_err(|_| YapError::NoLoggingWorker)?;
        Ok(())
    }
    pub(super) fn shutdown(&self) -> Result<(), ()> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        if self
            .command_tx
            .send(LoggingCommand::Shutdown(shutdown_tx))
            .is_ok()
        {
            if shutdown_rx.recv_timeout(Duration::from_secs(15)).is_ok() {
                Ok(())
            } else {
                error!("Logging thread didn't react to shutdown request.");
                Err(())
            }
        } else {
            error!("Couldn't send logging shutdown.");
            Err(())
        }
    }
}

impl LoggingWorker {
    fn work_loop(&mut self) -> Result<(), std::io::Error> {
        loop {
            match self.command_rx.recv() {
                Ok(LoggingCommand::Shutdown(sender)) => {
                    // TODO flush
                    self.close_files(true)?;
                    sender.send(()).unwrap();
                    break;
                }
                Ok(cmd) => self.handle_command(cmd)?,
                Err(e) => {
                    todo!();
                    break;
                }
            }
        }

        Ok(())
    }
    fn log_connection_event(
        &mut self,
        timestamp: DateTime<Local>,
        connected_to: Option<SerialPortInfo>,
    ) -> Result<(), std::io::Error> {
        if !self.settings.log_connection_events {
            return Ok(());
        }
        if let Some(text_file) = &mut self.text_file {
            if !self.last_rx_completed {
                write_line_ending(&mut text_file.inner)?;
                self.last_rx_completed = true;
            }

            let text = if let Some(port_info) = connected_to {
                let port_name = &port_info.port_name;
                format!("----- {timestamp} | Connected to {port_name}! -----")
            } else {
                format!("----- {timestamp} | Disconnected from port! -----")
            };
            text_file.inner.write_all(text.as_bytes())?;
            write_line_ending(&mut text_file.inner)?;
        }

        Ok(())
    }
    fn handle_command(&mut self, cmd: LoggingCommand) -> Result<(), std::io::Error> {
        match cmd {
            LoggingCommand::PortConnected(timestamp, port_info, reconnect_type) => {
                // if let Some(reconnect) = reconnect_type {
                // } else if self.settings.always_begin_on_connect {
                // }
                if reconnect_type.is_none() && self.settings.always_begin_on_connect {
                    self.begin_logging_session(&port_info)?;
                }
                self.log_connection_event(timestamp, Some(port_info))?;
            }
            LoggingCommand::RequestStart(port_info) => {
                assert!(self.raw_file.is_none());
                assert!(self.text_file.is_none());
                self.begin_logging_session(&port_info)?;
            }
            LoggingCommand::RxBytes(timestamp, buf) => {
                // let Some(raw_file) = &mut self.raw_file else {
                //     warn!("not logging byte buffer!");
                //     return Ok(());
                // };
                // raw_file.inner.write_all(&buf)?;
                if let Some(raw_file) = &mut self.raw_file {
                    raw_file.inner.write_all(&buf)?;
                };
                if let Some(text_file) = &mut self.text_file {
                    self.last_rx_completed = write_buffer_to_text_file(
                        timestamp,
                        &self.settings.timestamp,
                        &buf,
                        self.last_rx_completed,
                        &mut text_file.inner,
                        &self.line_ending,
                        // self.settings.timestamps,
                        LoggingLineType::RxLine,
                    )?;
                };
            }
            LoggingCommand::TxBytes {
                timestamp,
                bytes,
                line_ending,
            } => {
                let Some(text_file) = &mut self.text_file else {
                    warn!("not logging tx bytes, no text file!");
                    return Ok(());
                };
                if !self.settings.log_user_input {
                    warn!("not logging tx bytes, user log disabled!");
                    return Ok(());
                }
                self.last_rx_completed = write_buffer_to_text_file(
                    timestamp,
                    &self.settings.timestamp,
                    &bytes,
                    self.last_rx_completed,
                    &mut text_file.inner,
                    &self.line_ending,
                    // self.settings.timestamps,
                    LoggingLineType::TxLine { line_ending },
                )?;
            }
            LoggingCommand::LineEndingChange(whole_raw_buffer, new_ending) => todo!(),
            // LoggingCommand::RxTxLine(line) => {
            //     let Some(text_file) = &mut self.text_file else {
            //         warn!("not logging line!");
            //         return Ok(());
            //     };

            //     write_bufline_to_file(&mut text_file.inner, &line, self.settings.timestamps)?;
            // }
            LoggingCommand::RequestClearFiles => {
                if let Some(raw_file) = &mut self.raw_file {
                    raw_file.inner.set_len(raw_file.reset_index)?;
                    raw_file.inner.seek(SeekFrom::Start(raw_file.reset_index))?;
                }
                if let Some(text_file) = &mut self.text_file {
                    text_file.inner.set_len(text_file.reset_index)?;
                    text_file
                        .inner
                        .seek(SeekFrom::Start(text_file.reset_index))?;
                }
                self.flush_files(false)?;
            }
            // LoggingCommand::TxLine(line) => {
            //     let Some(text_file) = &mut self.text_file else {
            //         return Ok(());
            //     };

            //     write_bufline_to_file(text_file.inner, &line, self.settings.timestamps)?;
            // }
            LoggingCommand::Settings(new) => todo!(),
            LoggingCommand::RequestStop => {
                self.close_files(false)?;
            }
            LoggingCommand::PortDisconnect {
                timestamp,
                intentional: true,
            } => {
                if self.settings.close_file_on_disconnect {
                    self.close_files(false)?;
                } else {
                    self.flush_files(false)?;
                    self.update_reset_indices()?;
                }
            }
            LoggingCommand::PortDisconnect {
                timestamp,
                intentional: false,
            } => {
                self.log_connection_event(timestamp, None)?;
                self.flush_files(false)?;
            }
            LoggingCommand::Shutdown(sender) => unreachable!(),
        }
        Ok(())
    }

    fn begin_logging_session(&mut self, port_info: &SerialPortInfo) -> Result<(), std::io::Error> {
        self.session_open.store(true, Ordering::Release);
        let started_at = Local::now();
        self.create_log_files(started_at, port_info)?;
        self.started_at = Some(started_at);
        Ok(())
    }

    fn flush_files(&mut self, ignore_errors: bool) -> Result<(), std::io::Error> {
        let flush_file = |f: &mut fs::File| -> Result<(), std::io::Error> {
            match f.flush() {
                Ok(()) => (),
                Err(e) if ignore_errors => error!("Error flushing file, ignoring: {e}"),
                Err(e) => {
                    error!("Error flushing file: {e}");
                    return Err(e);
                }
            }
            match f.sync_all() {
                Ok(()) => (),
                Err(e) if ignore_errors => error!("Error flushing file, ignoring: {e}"),
                Err(e) => {
                    error!("Error flushing file: {e}");
                    return Err(e);
                }
            }
            Ok(())
        };
        if let Some(raw_file) = &mut self.raw_file {
            flush_file(&mut raw_file.inner)?;
        }
        if let Some(text_file) = &mut self.text_file {
            flush_file(&mut text_file.inner)?;
        }

        Ok(())
    }

    fn update_reset_indices(&mut self) -> Result<(), std::io::Error> {
        if let Some(raw_file) = &mut self.raw_file {
            let raw_file_metadata = fs::metadata(raw_file.inner.path())?;
            raw_file.reset_index = raw_file_metadata.len();
        }
        if let Some(text_file) = &mut self.text_file {
            let text_file_metadata = fs::metadata(text_file.inner.path())?;
            text_file.reset_index = text_file_metadata.len();
        }
        Ok(())
    }

    fn close_files(&mut self, ignore_errors: bool) -> Result<(), std::io::Error> {
        _ = self.started_at.take();
        self.session_open.store(false, Ordering::Release);

        self.flush_files(ignore_errors)?;

        _ = self.raw_file.take();
        _ = self.text_file.take();

        Ok(())
    }

    fn create_log_files(
        &mut self,
        started_at: DateTime<Local>,
        port_info: &SerialPortInfo,
    ) -> Result<(), std::io::Error> {
        let logs_dir = PathBuf::from("logs/");
        match logs_dir.try_exists() {
            Ok(true) if logs_dir.is_dir() => (),
            Ok(true) => todo!("handle not a dir?"),
            Ok(false) => fs::create_dir_all("logs/")?,
            Err(e) => {
                error!("Error checking for logs dir!");
                return Err(e);
            }
        }

        let make_binary_log = || -> Result<fs::File, std::io::Error> {
            let timestamp = started_at.format("logs/yap-%Y-%m-%d_%H-%M-%S.bin");

            fs::File::create(timestamp.to_string())
        };

        let make_text_log = |port_info: &SerialPortInfo| -> Result<fs::File, std::io::Error> {
            let timestamp = started_at.format("logs/yap-%Y-%m-%d_%H-%M-%S.txt");

            let port_text = match &port_info.port_type {
                serialport::SerialPortType::BluetoothPort => Cow::from("BT"),
                serialport::SerialPortType::PciPort => Cow::from("PCI"),
                serialport::SerialPortType::Unknown => Cow::from("Unknown"),
                serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                    pid,
                    vid,
                    serial_number: Some(serial),
                    ..
                }) => Cow::from(format!("USB ({pid:04X}:{vid:04X}:{serial})")),
                serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                    pid,
                    vid,
                    serial_number: None,
                    ..
                }) => Cow::from(format!("USB ({pid:04X}:{vid:04X})")),
            };

            let header_timestamp_format = if self.settings.timestamp.trim().is_empty() {
                FILE_LINE_TIMESTAMP_FORMAT
            } else {
                &self.settings.timestamp
            };

            let header_timestamp = started_at.format(header_timestamp_format);

            let file_header = format!(
                "----- {time} | Port: {name} | {port_text} -----",
                name = port_info.port_name,
                time = header_timestamp,
            );

            fs::File::create(timestamp.to_string())
                .and_then(|mut f| f.write_all(file_header.as_bytes()).map(|_| f))
                .and_then(|mut f| write_line_ending(&mut f).map(|_| f))
        };

        match self.settings.log_type {
            LoggingType::Both => {
                if self.raw_file.is_none() {
                    self.raw_file = Some(make_binary_log().map(Into::into)?);
                }
                if self.text_file.is_none() {
                    self.text_file = Some(make_text_log(port_info).map(Into::into)?);
                }
            }
            LoggingType::Raw => {
                // Raw only, flush and drop text file if we had one.
                if let Some(mut text_file) = self.text_file.take() {
                    text_file.inner.flush()?;
                    text_file.inner.sync_all()?;
                }
                if self.raw_file.is_none() {
                    self.raw_file = Some(make_binary_log().map(Into::into)?);
                }
            }
            LoggingType::Text => {
                // Text only, flush and drop raw binary file if we had one.
                if let Some(mut raw_file) = self.raw_file.take() {
                    raw_file.inner.flush()?;
                    raw_file.inner.sync_all()?;
                }
                if self.text_file.is_none() {
                    self.text_file = Some(make_text_log(port_info).map(Into::into)?);
                }
            }
        }

        Ok(())
    }
}

fn write_line_ending(file: &mut fs::File) -> Result<(), std::io::Error> {
    file.write_all(&[b'\n'])
}

fn write_buffer_to_text_file(
    timestamp: DateTime<Local>,
    timestamp_fmt: &str,
    bytes: &[u8],
    mut last_line_was_completed: bool,
    text_file: &mut fs::File,
    line_ending: &LineEnding,
    // with_timestamp: bool,
    line_type: LoggingLineType,
) -> Result<bool, std::io::Error> {
    let is_tx_line = matches!(&line_type, LoggingLineType::TxLine { .. });
    let timestamp_string = if timestamp_fmt.trim().is_empty() {
        None
    } else {
        Some(timestamp.format(timestamp_fmt).to_string())
    };

    for (_trunc, orig, _indices) in line_ending_iter(bytes, line_ending) {
        if last_line_was_completed || is_tx_line {
            let line_to_write = {
                let line_capacity = orig.len();
                let mut output = String::with_capacity(line_capacity);
                if let Some(timestamp_str) = &timestamp_string {
                    output.push_str(&timestamp_str);
                }
                if is_tx_line {
                    output.push_str("[USER] ")
                }

                output
            };
            if !last_line_was_completed && is_tx_line {
                write_line_ending(text_file)?;
            }
            text_file.write_all(line_to_write.as_bytes())?;
        }

        // for c in orig.escape_bytes() {
        //     let mut buf = [0; 4]; // Max bytes for any UTF-8 char
        //     let encoded = c.encode_utf8(&mut buf);
        //     text_file.write_all(encoded.as_bytes())?;
        // }

        for c in orig.escape_ascii() {
            text_file.write_all(&[c])?;
        }
        if let LoggingLineType::TxLine { line_ending } = &line_type {
            for c in line_ending.escape_ascii() {
                text_file.write_all(&[c])?;
            }
        }

        last_line_was_completed = orig.has_line_ending(line_ending) || is_tx_line;
        if last_line_was_completed {
            write_line_ending(text_file)?;
        }
    }

    let last_line_is_completed = last_line_was_completed;
    Ok(last_line_is_completed)
}

// fn write_bufline_to_file(
//     text_file: &mut fs::File,
//     line: &BufLine,
//     with_timestamp: bool,
// ) -> Result<(), std::io::Error> {
//     let line_capacity = line.value.iter().map(|s| s.content.len()).sum();

//     let line_to_write = {
//         let mut output = String::with_capacity(line_capacity);

//

//         // match line.line_type {
//         //     LineType::Port => (),
//         //     LineType::User {
//         //         is_macro: false, ..
//         //     } => output.push_str("(USER) "),
//         //     LineType::User { is_macro: true, .. } => output.push_str("(MACRO) "),
//         // }

//         line.value.iter().for_each(|s| output.push_str(&s.content));

//         output
//     };

//     text_file.write_all(line_to_write.as_bytes())?;
//     text_file.write_all(&[b'\n'])?;
//     Ok(())
// }
