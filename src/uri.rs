//! URI-based configuration for serial ports.
//!
//! # URI format
//!
//! Two forms are supported:
//!
//! ## Port-name form
//!
//! ```text
//! serial:///dev/ttyUSB0?baud=9600&parity=n
//! serial://COM3?baud=115200
//! ```
//!
//! The path component (`/dev/ttyUSB0`, `COM3`) is used as the serial port
//! device name.
//!
//! ## USB VID/PID form
//!
//! ```text
//! serial://?vid=0x2341&pid=0x0043
//! serial://?vid=0x2341&pid=0x0043&serial=DFSZGL
//! ```
//!
//! The matching serial port is discovered at runtime by scanning the available
//! ports and comparing USB identifiers.  This form is preferred because the
//! kernel device name (e.g. `/dev/ttyUSB0`) can change between reboots, while
//! the USB identifiers stay constant.
//!
//! # Query parameters
//!
//! | Key      | Values                                    | Default |
//! |----------|-------------------------------------------|---------|
//! | `baud`   | integer baud rate (e.g. `9600`, `115200`) | `9600`  |
//! | `parity` | `n` / `none`, `e` / `even`, `o` / `odd`  | `n`     |
//! | `data`   | `5`, `6`, `7`, `8`                        | `8`     |
//! | `stop`   | `1`, `2`                                  | `1`     |
//! | `flow`   | `none`, `hw` / `hardware`, `sw` / `software` | `none` |
//! | `vid`    | USB vendor ID (hex, e.g. `0x2341`)        | —       |
//! | `pid`    | USB product ID (hex, e.g. `0x0043`)       | —       |
//! | `serial` | USB serial-number string                  | —       |

use std::fmt;

use serialport::{DataBits, FlowControl, Parity, SerialPortType, StopBits};

// ---------------------------------------------------------------------------
// ParseUriError
// ---------------------------------------------------------------------------

/// Errors that can occur while parsing a serial URI.
#[derive(Debug, thiserror::Error)]
pub enum ParseUriError {
    /// The URI does not start with the `serial://` scheme.
    #[error("invalid URI scheme: expected \"serial://\"")]
    InvalidScheme,

    /// A query parameter has an unrecognised or malformed value.
    #[error("invalid query parameter \"{key}\": {reason}")]
    InvalidParam { key: String, reason: String },

    /// No matching serial port could be found for the given USB identifiers.
    #[error("no serial port found matching {0}")]
    PortNotFound(String),
}

// ---------------------------------------------------------------------------
// SerialUri
// ---------------------------------------------------------------------------

/// Parsed representation of a serial URI.
///
/// Create one via [`SerialUri::parse`] then resolve the port name (and
/// optionally enumerate connected devices) via [`SerialUri::resolve_port`].
#[derive(Debug, Clone)]
pub struct SerialUri {
    /// Explicit port name supplied in the URI path (e.g. `/dev/ttyUSB0`).
    ///
    /// `None` when the URI uses USB VID/PID discovery instead.
    pub port_name: Option<String>,

    /// Baud rate in bits per second.  Defaults to `9600`.
    pub baud_rate: u32,

    /// Parity setting.  Defaults to [`Parity::None`].
    pub parity: Parity,

    /// Number of data bits.  Defaults to [`DataBits::Eight`].
    pub data_bits: DataBits,

    /// Stop-bit configuration.  Defaults to [`StopBits::One`].
    pub stop_bits: StopBits,

    /// Flow-control mode.  Defaults to [`FlowControl::None`].
    pub flow_control: FlowControl,

    /// USB vendor ID for device discovery.
    pub vid: Option<u16>,

    /// USB product ID for device discovery.
    pub pid: Option<u16>,

    /// USB serial-number string for device discovery.
    pub usb_serial: Option<String>,
}

impl SerialUri {
    /// Parse a serial URI string into a [`SerialUri`].
    ///
    /// # Errors
    ///
    /// Returns [`ParseUriError::InvalidScheme`] when the URI does not start
    /// with `serial://`, or [`ParseUriError::InvalidParam`] when a query
    /// parameter contains an unrecognised value.
    ///
    /// # Examples
    ///
    /// ```
    /// use lulu_serial_port::SerialUri;
    ///
    /// let cfg = SerialUri::parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
    /// assert_eq!(cfg.port_name.as_deref(), Some("/dev/ttyUSB0"));
    /// assert_eq!(cfg.baud_rate, 115_200);
    ///
    /// let cfg = SerialUri::parse("serial://?vid=0x2341&pid=0x0043").unwrap();
    /// assert_eq!(cfg.vid, Some(0x2341));
    /// assert_eq!(cfg.pid, Some(0x0043));
    /// ```
    pub fn parse(uri: &str) -> Result<Self, ParseUriError> {
        // Strip the mandatory "serial://" prefix.
        let rest = uri
            .strip_prefix("serial://")
            .ok_or(ParseUriError::InvalidScheme)?;

        // Split on '?' to separate path from query string.
        let (path, query) = match rest.split_once('?') {
            Some((p, q)) => (p, q),
            None => (rest, ""),
        };

        // The path component is the port name.  An empty path means no port
        // name was supplied (VID/PID-only URI).
        let port_name: Option<String> = if path.is_empty() {
            None
        } else {
            Some(path.to_string())
        };

        // Defaults
        let mut baud_rate: u32 = 9_600;
        let mut parity = Parity::None;
        let mut data_bits = DataBits::Eight;
        let mut stop_bits = StopBits::One;
        let mut flow_control = FlowControl::None;
        let mut vid: Option<u16> = None;
        let mut pid: Option<u16> = None;
        let mut usb_serial: Option<String> = None;

        // Parse each "key=value" pair in the query string.
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "baud" | "baud_rate" => {
                    baud_rate = value.parse::<u32>().map_err(|_| ParseUriError::InvalidParam {
                        key: key.to_string(),
                        reason: format!("\"{}\" is not a valid integer", value),
                    })?;
                }
                "parity" => {
                    parity = parse_parity(value).map_err(|reason| ParseUriError::InvalidParam {
                        key: key.to_string(),
                        reason,
                    })?;
                }
                "data" => {
                    data_bits =
                        parse_data_bits(value).map_err(|reason| ParseUriError::InvalidParam {
                            key: key.to_string(),
                            reason,
                        })?;
                }
                "stop" => {
                    stop_bits =
                        parse_stop_bits(value).map_err(|reason| ParseUriError::InvalidParam {
                            key: key.to_string(),
                            reason,
                        })?;
                }
                "flow" => {
                    flow_control =
                        parse_flow_control(value).map_err(|reason| ParseUriError::InvalidParam {
                            key: key.to_string(),
                            reason,
                        })?;
                }
                "vid" => {
                    vid = Some(parse_u16_hex(value).map_err(|reason| {
                        ParseUriError::InvalidParam {
                            key: key.to_string(),
                            reason,
                        }
                    })?);
                }
                "pid" => {
                    pid = Some(parse_u16_hex(value).map_err(|reason| {
                        ParseUriError::InvalidParam {
                            key: key.to_string(),
                            reason,
                        }
                    })?);
                }
                "serial" => {
                    usb_serial = Some(value.to_string());
                }
                _ => {
                    // Ignore unknown parameters for forward-compatibility.
                }
            }
        }

        Ok(Self {
            port_name,
            baud_rate,
            parity,
            data_bits,
            stop_bits,
            flow_control,
            vid,
            pid,
            usb_serial,
        })
    }

    /// Resolve the concrete serial port name to use for this configuration.
    ///
    /// When USB VID/PID identifiers are present they take priority over the
    /// [`port_name`][Self::port_name] field: the system's available serial
    /// ports are enumerated and the first matching device is returned.
    ///
    /// If no USB identifiers are present and [`port_name`][Self::port_name] is
    /// set, that name is returned directly (no I/O).
    ///
    /// # Errors
    ///
    /// Returns [`ParseUriError::PortNotFound`] when USB identifiers were
    /// provided but no matching port was found, or when neither USB identifiers
    /// nor a port name are present.
    pub fn resolve_port(&self) -> Result<String, ParseUriError> {
        // USB identifiers take priority.
        if self.vid.is_some() || self.pid.is_some() {
            return self.find_usb_port();
        }

        // Fall back to the explicit port name.
        self.port_name
            .clone()
            .ok_or_else(|| ParseUriError::PortNotFound("no port name or USB identifiers provided".to_string()))
    }

    /// Enumerate available ports and find the first one whose USB identifiers
    /// match those stored in this configuration.
    fn find_usb_port(&self) -> Result<String, ParseUriError> {
        let ports = serialport::available_ports().map_err(|e| {
            ParseUriError::PortNotFound(format!("cannot enumerate ports: {}", e))
        })?;

        for info in &ports {
            if let SerialPortType::UsbPort(usb) = &info.port_type {
                let vid_match = self.vid.is_none_or(|v| v == usb.vid);
                let pid_match = self.pid.is_none_or(|v| v == usb.pid);
                let serial_match = match (&self.usb_serial, &usb.serial_number) {
                    (Some(expected), Some(actual)) => expected == actual,
                    (Some(_), None) => false,
                    (None, _) => true,
                };

                if vid_match && pid_match && serial_match {
                    return Ok(info.port_name.clone());
                }
            }
        }

        Err(ParseUriError::PortNotFound(self.usb_description()))
    }

    /// Build a human-readable description of the USB identifiers.
    fn usb_description(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.vid {
            parts.push(format!("vid=0x{:04x}", v));
        }
        if let Some(p) = self.pid {
            parts.push(format!("pid=0x{:04x}", p));
        }
        if let Some(s) = &self.usb_serial {
            parts.push(format!("serial={}", s));
        }
        parts.join(" ")
    }

    /// Return the URI string representation of this configuration.
    ///
    /// The returned string can be passed back to [`SerialUri::parse`].
    pub fn to_uri(&self) -> String {
        let mut uri = "serial://".to_string();

        if let Some(port) = &self.port_name {
            uri.push_str(port);
        }

        let mut params: Vec<String> = Vec::new();

        if let Some(v) = self.vid {
            params.push(format!("vid=0x{:04x}", v));
        }
        if let Some(p) = self.pid {
            params.push(format!("pid=0x{:04x}", p));
        }
        if let Some(s) = &self.usb_serial {
            params.push(format!("serial={}", s));
        }

        if self.baud_rate != 9_600 {
            params.push(format!("baud={}", self.baud_rate));
        }

        let parity_str = match self.parity {
            Parity::None => None,
            Parity::Even => Some("e"),
            Parity::Odd => Some("o"),
        };
        if let Some(p) = parity_str {
            params.push(format!("parity={}", p));
        }

        if !matches!(self.data_bits, DataBits::Eight) {
            let d = match self.data_bits {
                DataBits::Five => 5,
                DataBits::Six => 6,
                DataBits::Seven => 7,
                DataBits::Eight => 8,
            };
            params.push(format!("data={}", d));
        }

        if !matches!(self.stop_bits, StopBits::One) {
            params.push("stop=2".to_string());
        }

        let flow_str = match self.flow_control {
            FlowControl::None => None,
            FlowControl::Hardware => Some("hw"),
            FlowControl::Software => Some("sw"),
        };
        if let Some(f) = flow_str {
            params.push(format!("flow={}", f));
        }

        if !params.is_empty() {
            uri.push('?');
            uri.push_str(&params.join("&"));
        }

        uri
    }
}

impl fmt::Display for SerialUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uri())
    }
}

// ---------------------------------------------------------------------------
// Free-standing parse helper
// ---------------------------------------------------------------------------

/// Parse a serial URI string and return a [`SerialUri`].
///
/// This is a convenience wrapper around [`SerialUri::parse`].
///
/// # Errors
///
/// See [`ParseUriError`].
pub fn parse_serial_uri(uri: &str) -> Result<SerialUri, ParseUriError> {
    SerialUri::parse(uri)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn parse_parity(s: &str) -> Result<Parity, String> {
    match s.to_lowercase().as_str() {
        "n" | "none" => Ok(Parity::None),
        "e" | "even" => Ok(Parity::Even),
        "o" | "odd" => Ok(Parity::Odd),
        _ => Err(format!(
            "\"{}\" is not a valid parity (use n/none, e/even, o/odd)",
            s
        )),
    }
}

fn parse_data_bits(s: &str) -> Result<DataBits, String> {
    match s {
        "5" => Ok(DataBits::Five),
        "6" => Ok(DataBits::Six),
        "7" => Ok(DataBits::Seven),
        "8" => Ok(DataBits::Eight),
        _ => Err(format!("\"{}\" is not a valid data-bit count (use 5, 6, 7 or 8)", s)),
    }
}

fn parse_stop_bits(s: &str) -> Result<StopBits, String> {
    match s {
        "1" => Ok(StopBits::One),
        "2" => Ok(StopBits::Two),
        _ => Err(format!("\"{}\" is not a valid stop-bit count (use 1 or 2)", s)),
    }
}

fn parse_flow_control(s: &str) -> Result<FlowControl, String> {
    match s.to_lowercase().as_str() {
        "none" => Ok(FlowControl::None),
        "hw" | "hardware" => Ok(FlowControl::Hardware),
        "sw" | "software" => Ok(FlowControl::Software),
        _ => Err(format!(
            "\"{}\" is not a valid flow-control mode (use none, hw/hardware, sw/software)",
            s
        )),
    }
}

fn parse_u16_hex(s: &str) -> Result<u16, String> {
    let stripped = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(stripped, 16)
        .map_err(|_| format!("\"{}\" is not a valid 16-bit hex value", s))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_port_name_and_baud() {
        let uri = SerialUri::parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(uri.port_name.as_deref(), Some("/dev/ttyUSB0"));
        assert_eq!(uri.baud_rate, 115_200);
        assert!(matches!(uri.parity, Parity::None));
    }

    #[test]
    fn parse_windows_port() {
        let uri = SerialUri::parse("serial://COM3?baud=9600&parity=e").unwrap();
        assert_eq!(uri.port_name.as_deref(), Some("COM3"));
        assert_eq!(uri.baud_rate, 9_600);
        assert!(matches!(uri.parity, Parity::Even));
    }

    #[test]
    fn parse_usb_vid_pid() {
        let uri = SerialUri::parse("serial://?vid=0x2341&pid=0x0043").unwrap();
        assert_eq!(uri.vid, Some(0x2341));
        assert_eq!(uri.pid, Some(0x0043));
        assert!(uri.port_name.is_none());
    }

    #[test]
    fn parse_usb_with_serial_number() {
        let uri =
            SerialUri::parse("serial://?vid=0x2341&pid=0x0043&serial=DFSZGL").unwrap();
        assert_eq!(uri.vid, Some(0x2341));
        assert_eq!(uri.pid, Some(0x0043));
        assert_eq!(uri.usb_serial.as_deref(), Some("DFSZGL"));
    }

    #[test]
    fn parse_all_params() {
        let uri = SerialUri::parse(
            "serial:///dev/ttyS0?baud=4800&parity=o&data=7&stop=2&flow=hw",
        )
        .unwrap();
        assert_eq!(uri.baud_rate, 4_800);
        assert!(matches!(uri.parity, Parity::Odd));
        assert!(matches!(uri.data_bits, DataBits::Seven));
        assert!(matches!(uri.stop_bits, StopBits::Two));
        assert!(matches!(uri.flow_control, FlowControl::Hardware));
    }

    #[test]
    fn default_baud_rate() {
        let uri = SerialUri::parse("serial:///dev/ttyUSB0").unwrap();
        assert_eq!(uri.baud_rate, 9_600);
    }

    #[test]
    fn invalid_scheme() {
        assert!(SerialUri::parse("uart:///dev/ttyUSB0").is_err());
    }

    #[test]
    fn invalid_baud() {
        assert!(SerialUri::parse("serial:///dev/ttyUSB0?baud=notanumber").is_err());
    }

    #[test]
    fn invalid_parity() {
        assert!(SerialUri::parse("serial:///dev/ttyUSB0?parity=x").is_err());
    }

    #[test]
    fn invalid_vid_hex() {
        assert!(SerialUri::parse("serial://?vid=notahex").is_err());
    }

    #[test]
    fn resolve_port_from_name() {
        let uri = SerialUri::parse("serial:///dev/ttyUSB0").unwrap();
        assert_eq!(uri.resolve_port().unwrap(), "/dev/ttyUSB0");
    }

    #[test]
    fn resolve_port_no_info() {
        let uri = SerialUri::parse("serial://").unwrap();
        assert!(uri.resolve_port().is_err());
    }

    #[test]
    fn to_uri_roundtrip_port_name() {
        let original = "serial:///dev/ttyUSB0?baud=115200";
        let uri = SerialUri::parse(original).unwrap();
        assert_eq!(uri.to_uri(), original);
    }

    #[test]
    fn to_uri_defaults_omitted() {
        let uri = SerialUri::parse("serial:///dev/ttyUSB0").unwrap();
        // Default baud (9600) and parity (n) are omitted from the output.
        assert_eq!(uri.to_uri(), "serial:///dev/ttyUSB0");
    }

    #[test]
    fn to_uri_usb_identifiers() {
        let original = "serial://?vid=0x2341&pid=0x0043&serial=DFSZGL";
        let uri = SerialUri::parse(original).unwrap();
        assert_eq!(uri.to_uri(), original);
    }

    #[test]
    fn display_matches_to_uri() {
        let uri = SerialUri::parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(format!("{}", uri), uri.to_uri());
    }

    #[test]
    fn parse_serial_uri_fn() {
        let uri = parse_serial_uri("serial:///dev/ttyS1?baud=38400").unwrap();
        assert_eq!(uri.baud_rate, 38_400);
    }

    #[test]
    fn uppercase_hex_vid() {
        let uri = SerialUri::parse("serial://?vid=0X2341&pid=0X0043").unwrap();
        assert_eq!(uri.vid, Some(0x2341));
        assert_eq!(uri.pid, Some(0x0043));
    }
}
