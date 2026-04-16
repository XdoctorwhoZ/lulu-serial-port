//! # lulu-serial-port
//!
//! A Rust library to manage serial ports with MQTT integration for
//! automated testing.
//!
//! The [`SerialPortManager`] allows you to:
//! - Open a serial port and read incoming data asynchronously, line by line.
//! - Log every received and every sent message to an MQTT broker.
//! - Wait asynchronously for a specific pattern to appear in incoming data
//!   (with a configurable timeout).
//!
//! # Quick-start
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use lulu_serial_port::{MqttConfig, SerialPortConfig, SerialPortManager};
//!
//! #[tokio::main]
//! async fn main() {
//!     let mqtt_config = MqttConfig {
//!         broker_host: "127.0.0.1".to_string(),
//!         broker_port: 1883,
//!         ..Default::default()
//!     };
//!
//!     let serial_config = SerialPortConfig {
//!         port_name: "/dev/ttyUSB0".to_string(),
//!         baud_rate: 115_200,
//!         lulu_source: "serial/my-device".to_string(),
//!         lulu_rx_attribute: "rx".to_string(),
//!         lulu_tx_attribute: "tx".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let manager = SerialPortManager::new(serial_config, mqtt_config)
//!         .await
//!         .unwrap();
//!
//!     // Send a command to the device.
//!     manager.send(b"AT\r\n").await.unwrap();
//!
//!     // Block until the device replies with a line containing "OK",
//!     // or give up after 5 seconds.
//!     let response = manager
//!         .wait_for("OK", Duration::from_secs(5))
//!         .await
//!         .unwrap();
//!
//!     println!("Got: {}", response);
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio_serial::SerialPortBuilderExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Mutex};
use tokio::time::timeout;

mod error;
mod mqtt;
mod uri;

pub use error::Error;
pub use mqtt::MqttConfig;
pub use uri::{parse_serial_uri, ParseUriError, SerialUri};
// Re-export serialport line-parameter types for convenience.
pub use serialport::{DataBits, FlowControl, Parity, StopBits};

// ---------------------------------------------------------------------------
// SerialPortConfig
// ---------------------------------------------------------------------------

/// Configuration for a [`SerialPortManager`].
#[derive(Debug, Clone)]
pub struct SerialPortConfig {
    /// Serial port device path (e.g. `/dev/ttyUSB0` on Linux, `COM3` on Windows).
    pub port_name: String,

    /// Baud rate in bits per second (e.g. `115_200`, `9_600`).
    pub baud_rate: u32,

    /// Parity setting.  Defaults to [`Parity::None`].
    pub parity: Parity,

    /// Number of data bits per frame.  Defaults to [`DataBits::Eight`].
    pub data_bits: DataBits,

    /// Stop-bit configuration.  Defaults to [`StopBits::One`].
    pub stop_bits: StopBits,

    /// Flow-control mode.  Defaults to [`FlowControl::None`].
    pub flow_control: FlowControl,

    /// MQTT source segments for this device (e.g. `"serial/my-device"`).
    ///
    /// This value is used as the `source` argument in all MQTT publish calls
    /// made by the manager.  The resulting MQTT topic has the form
    /// `lulu/{source}/{attribute}`.
    pub lulu_source: String,

    /// MQTT attribute name for received (RX) messages (e.g. `"rx"`).
    pub lulu_rx_attribute: String,

    /// MQTT attribute name for transmitted (TX) messages (e.g. `"tx"`).
    pub lulu_tx_attribute: String,
}

impl Default for SerialPortConfig {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 9_600,
            parity: Parity::None,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
            lulu_source: String::new(),
            lulu_rx_attribute: "rx".to_string(),
            lulu_tx_attribute: "tx".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// SerialPortManager
// ---------------------------------------------------------------------------

/// Manages an open serial port with asynchronous I/O and MQTT integration.
///
/// Dropping the manager aborts the background reader task and closes the port.
pub struct SerialPortManager {
    writer: Arc<Mutex<tokio::io::WriteHalf<tokio_serial::SerialStream>>>,
    /// Sender side of the broadcast channel — used both to publish messages and
    /// to create new [`broadcast::Receiver`] handles for [`Self::wait_for`].
    broadcast_tx: broadcast::Sender<String>,
    mqtt: mqtt::MqttHandle,
    lulu_source: String,
    lulu_tx_attribute: String,
    /// Abort handle for the background reader task; cancelled on [`Drop`].
    _reader_task_abort: tokio::task::AbortHandle,
}

impl SerialPortManager {
    /// Opens a serial port described by a serial URI and starts the background
    /// reader task.
    ///
    /// This is a convenience constructor that combines [`SerialUri::parse`],
    /// [`SerialUri::resolve_port`] and [`Self::new`].  The serial URI supplies
    /// the port name (or USB VID/PID for automatic discovery) and the line
    /// parameters (baud rate, parity, …).  The MQTT routing fields must be
    /// supplied separately because they are not part of the URI.
    ///
    /// # Arguments
    ///
    /// * `uri` — serial URI string (see [`SerialUri`]).
    /// * `lulu_source` — MQTT source path (e.g. `"serial/my-device"`).
    /// * `lulu_rx_attribute` — MQTT attribute for received data (e.g. `"rx"`).
    /// * `lulu_tx_attribute` — MQTT attribute for transmitted data (e.g. `"tx"`).
    /// * `mqtt_config` — MQTT client configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Uri`] when the URI cannot be parsed or no matching
    /// port is found, [`Error::SerialPort`] when the port cannot be opened,
    /// and [`Error::Mqtt`] when the MQTT connection fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use lulu_serial_port::{MqttConfig, SerialPortManager};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mqtt_config = MqttConfig::default();
    ///
    ///     // Open by device name
    ///     let _mgr = SerialPortManager::from_uri(
    ///         "serial:///dev/ttyUSB0?baud=115200",
    ///         "serial/my-device",
    ///         "rx",
    ///         "tx",
    ///         mqtt_config.clone(),
    ///     )
    ///     .await
    ///     .unwrap();
    ///
    ///     // Open by USB VID/PID (preferred — survives reboots)
    ///     let _mgr2 = SerialPortManager::from_uri(
    ///         "serial://?vid=0x2341&pid=0x0043&baud=115200",
    ///         "serial/my-device",
    ///         "rx",
    ///         "tx",
    ///         MqttConfig::default(),
    ///     )
    ///     .await
    ///     .unwrap();
    /// }
    /// ```
    pub async fn from_uri(
        uri: &str,
        lulu_source: impl Into<String>,
        lulu_rx_attribute: impl Into<String>,
        lulu_tx_attribute: impl Into<String>,
        mqtt_config: MqttConfig,
    ) -> Result<Self, Error> {
        let serial_uri = SerialUri::parse(uri)?;
        let port_name = serial_uri.resolve_port()?;

        let serial_config = SerialPortConfig {
            port_name,
            baud_rate: serial_uri.baud_rate,
            parity: serial_uri.parity,
            data_bits: serial_uri.data_bits,
            stop_bits: serial_uri.stop_bits,
            flow_control: serial_uri.flow_control,
            lulu_source: lulu_source.into(),
            lulu_rx_attribute: lulu_rx_attribute.into(),
            lulu_tx_attribute: lulu_tx_attribute.into(),
        };

        Self::new(serial_config, mqtt_config).await
    }

    /// Opens `serial_config.port_name` at `serial_config.baud_rate` and starts
    /// the background reader task.
    ///
    /// # MQTT initialisation
    ///
    /// An MQTT client is created and connected to the broker described by
    /// `mqtt_config`.  The connection is maintained in the background; if it
    /// drops the client will reconnect automatically with exponential
    /// back-off.
    ///
    /// # Errors
    ///
    /// - [`Error::SerialPort`] — the port cannot be opened (wrong path, in use,
    ///   wrong permissions, …).
    /// - [`Error::Mqtt`] — the MQTT client could not be created.
    pub async fn new(
        serial_config: SerialPortConfig,
        mqtt_config: MqttConfig,
    ) -> Result<Self, Error> {
        // Connect to the MQTT broker.
        let mqtt = mqtt::MqttHandle::connect(&mqtt_config).await?;

        // Open the serial port with all configured line parameters.
        let port = tokio_serial::new(&serial_config.port_name, serial_config.baud_rate)
            .parity(serial_config.parity)
            .data_bits(serial_config.data_bits)
            .stop_bits(serial_config.stop_bits)
            .flow_control(serial_config.flow_control)
            .open_native_async()
            .map_err(|e| Error::SerialPort(e.to_string()))?;

        // Split into independent read / write halves so the writer can be held
        // behind a mutex while the reader lives inside the background task.
        let (reader_half, writer_half) = tokio::io::split(port);
        let writer = Arc::new(Mutex::new(writer_half));

        // Broadcast channel: the background task is the single producer; each
        // wait_for() call creates a fresh receiver.
        let (broadcast_tx, _initial_rx) = broadcast::channel::<String>(256);
        let broadcast_tx_for_task = broadcast_tx.clone();

        // Clone MQTT handle and routing info for the background task.
        let source = serial_config.lulu_source.clone();
        let rx_attr = serial_config.lulu_rx_attribute.clone();
        let mqtt_for_task = mqtt.clone();

        // Spawn the background reader task.
        let join_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(reader_half);
            let mut line = String::new();

            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    // EOF — port was closed cleanly.
                    Ok(0) => {
                        tracing::info!("serial port EOF — reader task exiting");
                        break;
                    }
                    Ok(_) => {
                        // Strip the line terminator(s) before publishing.
                        let msg = line
                            .trim_end_matches(['\r', '\n'])
                            .to_string();

                        tracing::debug!("serial RX: {:?}", msg);

                        // Publish to MQTT (best-effort — ignore errors).
                        mqtt_for_task.publish(&source, &rx_attr, &msg).await;

                        // Broadcast to any active wait_for() subscribers.
                        // A send error just means there are no current receivers.
                        let _ = broadcast_tx_for_task.send(msg);
                    }
                    Err(e) => {
                        tracing::error!("serial read error: {} — reader task exiting", e);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            writer,
            broadcast_tx,
            mqtt,
            lulu_source: serial_config.lulu_source,
            lulu_tx_attribute: serial_config.lulu_tx_attribute,
            _reader_task_abort: join_handle.abort_handle(),
        })
    }

    /// Writes `data` to the serial port and logs it to MQTT.
    ///
    /// The bytes are logged as a UTF-8 string (invalid bytes are replaced with
    /// the Unicode replacement character `\u{FFFD}`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the underlying write fails.
    pub async fn send(&self, data: &[u8]) -> Result<(), Error> {
        // Log to MQTT (best-effort — a missing MQTT connection should not
        // prevent the serial write from happening).
        let data_str = String::from_utf8_lossy(data).into_owned();
        self.mqtt
            .publish(&self.lulu_source, &self.lulu_tx_attribute, &data_str)
            .await;

        tracing::debug!("serial TX: {} bytes", data.len());

        let mut writer = self.writer.lock().await;
        writer.write_all(data).await?;
        Ok(())
    }

    /// Waits asynchronously for the first received line that contains `pattern`.
    ///
    /// The function subscribes to the internal broadcast channel so it is safe
    /// to call concurrently from multiple tasks (each caller gets its own view
    /// of the message stream).
    ///
    /// # Arguments
    ///
    /// * `pattern` — substring to search for in incoming lines.
    /// * `timeout_duration` — maximum time to wait; if no matching line arrives
    ///   within this duration [`Error::Timeout`] is returned.
    ///
    /// # Returns
    ///
    /// The first complete line (without the trailing newline) that contains
    /// `pattern`.
    ///
    /// # Errors
    ///
    /// - [`Error::Timeout`] — no matching line was received before the deadline.
    /// - [`Error::ChannelClosed`] — the background reader task has stopped
    ///   (e.g. because the port was physically disconnected).
    pub async fn wait_for(
        &self,
        pattern: &str,
        timeout_duration: Duration,
    ) -> Result<String, Error> {
        let mut rx = self.broadcast_tx.subscribe();
        let pattern = pattern.to_string();

        timeout(timeout_duration, async move {
            loop {
                match rx.recv().await {
                    Ok(msg) if msg.contains(pattern.as_str()) => return Ok(msg),
                    Ok(_) => {
                        // Line received but does not match — keep waiting.
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Some messages were dropped because this consumer was too
                        // slow; continue without treating this as a fatal error.
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(Error::ChannelClosed);
                    }
                }
            }
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

impl Drop for SerialPortManager {
    /// Aborts the background reader task when the manager is dropped.
    fn drop(&mut self) {
        self._reader_task_abort.abort();
    }
}
