//! # lulu-serial-port
//!
//! A Rust library to manage serial ports with [lulu-logs] integration for
//! automated testing.
//!
//! The [`SerialPortManager`] allows you to:
//! - Open a serial port and read incoming data asynchronously, line by line.
//! - Log every received and every sent message to a lulu-logs MQTT broker.
//! - Wait asynchronously for a specific pattern to appear in incoming data
//!   (with a configurable timeout).
//!
//! # Quick-start
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use lulu_serial_port::{SerialPortConfig, SerialPortManager};
//! use lulu_logs_client::LuluClientConfig;
//!
//! #[tokio::main]
//! async fn main() {
//!     let lulu_config = LuluClientConfig {
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
//!     };
//!
//!     let manager = SerialPortManager::new(serial_config, lulu_config)
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
//!
//! [lulu-logs]: https://github.com/XdoctorwhoZ/lulu-logs

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Mutex};
use tokio::time::timeout;
use tokio_serial::SerialPortBuilderExt;

mod error;

pub use error::Error;
// Re-export lulu-logs types so users do not need to depend on lulu-logs-client directly.
pub use lulu_logs_client::{Data, LogLevel, LuluClientConfig};

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

    /// lulu-logs source segments for this device (e.g. `"serial/my-device"`).
    ///
    /// This value is used as the `source` argument in all `lulu_publish` calls
    /// made by the manager.  It must follow the lulu-logs naming rules
    /// (lower-case alphanumeric segments separated by `/`, with `-` allowed
    /// within a segment).
    pub lulu_source: String,

    /// lulu-logs attribute name for received (RX) messages (e.g. `"rx"`).
    pub lulu_rx_attribute: String,

    /// lulu-logs attribute name for transmitted (TX) messages (e.g. `"tx"`).
    pub lulu_tx_attribute: String,
}

// ---------------------------------------------------------------------------
// SerialPortManager
// ---------------------------------------------------------------------------

/// Manages an open serial port with asynchronous I/O and lulu-logs integration.
///
/// Dropping the manager aborts the background reader task and closes the port.
pub struct SerialPortManager {
    writer: Arc<Mutex<tokio::io::WriteHalf<tokio_serial::SerialStream>>>,
    /// Sender side of the broadcast channel — used both to publish messages and
    /// to create new [`broadcast::Receiver`] handles for [`Self::wait_for`].
    broadcast_tx: broadcast::Sender<String>,
    lulu_source: String,
    lulu_tx_attribute: String,
    /// Abort handle for the background reader task; cancelled on [`Drop`].
    _reader_task_abort: tokio::task::AbortHandle,
}

impl SerialPortManager {
    /// Opens `serial_config.port_name` at `serial_config.baud_rate` and starts
    /// the background reader task.
    ///
    /// # lulu-logs initialisation
    ///
    /// If lulu-logs has not yet been initialised the manager calls
    /// [`lulu_logs_client::lulu_init`] with `lulu_config`.  If it was already
    /// initialised (e.g. by another component or a previous call) `lulu_config`
    /// is silently ignored.
    ///
    /// # Errors
    ///
    /// - [`Error::SerialPort`] — the port cannot be opened (wrong path, in use,
    ///   wrong permissions, …).
    /// - [`Error::LuluLogs`] — lulu-logs could not be initialised.
    pub async fn new(
        serial_config: SerialPortConfig,
        lulu_config: LuluClientConfig,
    ) -> Result<Self, Error> {
        // Initialise lulu-logs (idempotent — AlreadyInitialized is not an error).
        match lulu_logs_client::lulu_init(lulu_config) {
            Ok(()) | Err(lulu_logs_client::LuluError::AlreadyInitialized) => {}
            Err(e) => return Err(Error::LuluLogs(e.to_string())),
        }

        // Open the serial port.
        let port = tokio_serial::new(&serial_config.port_name, serial_config.baud_rate)
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

        // Clone lulu-logs routing info for the background task.
        let source = serial_config.lulu_source.clone();
        let rx_attr = serial_config.lulu_rx_attribute.clone();

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

                        // Publish to lulu-logs (best-effort — ignore errors).
                        let _ = lulu_logs_client::lulu_publish(
                            &source,
                            &rx_attr,
                            lulu_logs_client::LogLevel::Info,
                            lulu_logs_client::Data::String(msg.clone()),
                        );

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
            lulu_source: serial_config.lulu_source,
            lulu_tx_attribute: serial_config.lulu_tx_attribute,
            _reader_task_abort: join_handle.abort_handle(),
        })
    }

    /// Writes `data` to the serial port and logs it to lulu-logs.
    ///
    /// The bytes are logged as a UTF-8 string (invalid bytes are replaced with
    /// the Unicode replacement character `\u{FFFD}`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the underlying write fails.
    pub async fn send(&self, data: &[u8]) -> Result<(), Error> {
        // Log to lulu-logs (best-effort — a missing MQTT connection should not
        // prevent the serial write from happening).
        let data_str = String::from_utf8_lossy(data).into_owned();
        let _ = lulu_logs_client::lulu_publish(
            &self.lulu_source,
            &self.lulu_tx_attribute,
            lulu_logs_client::LogLevel::Info,
            lulu_logs_client::Data::String(data_str),
        );

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
