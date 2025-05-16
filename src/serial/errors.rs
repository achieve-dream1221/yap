#[derive(Debug, thiserror::Error)]
pub(crate) enum SerialError {
    #[error("serial port error: {0}")]
    SerialPort(#[from] serialport::Error),
    #[error("no parent app reciever to send to")]
    FailedSend,

    #[cfg(feature = "espflash")]
    #[error("espflash error: {0}")]
    EspFlash(#[from] espflash::error::Error),
    #[cfg(feature = "espflash")]
    #[error("tried to act on lent out port")]
    MissingPort,
}
