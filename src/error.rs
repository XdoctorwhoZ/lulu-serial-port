/// Errors returned by the `lulu-serial-port` public API.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to open or communicate with the serial port.
    #[error("serial port error: {0}")]
    SerialPort(String),

    /// An I/O error occurred while writing to the serial port.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// [`crate::SerialPortManager::wait_for`] timed out before the pattern was received.
    #[error("wait_for timed out")]
    Timeout,

    /// The background reader task has stopped (serial port closed or errored).
    #[error("serial port reader task closed")]
    ChannelClosed,

    /// An error occurred while initialising or communicating with the MQTT broker.
    #[error("MQTT error: {0}")]
    Mqtt(String),

    /// Failed to parse or resolve a serial URI.
    #[error("URI error: {0}")]
    Uri(#[from] crate::ParseUriError),
}
