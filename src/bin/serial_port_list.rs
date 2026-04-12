//! `serial-port-list` — List all available serial ports and their URIs.
//!
//! # Usage
//!
//! ```text
//! serial-port-list
//! ```
//!
//! # Example output
//!
//! ```text
//! /dev/ttyUSB0  serial:///dev/ttyUSB0?vid=0x2341&pid=0x0043&serial=DFSZGL
//! /dev/ttyS0    serial:///dev/ttyS0
//! ```
//!
//! USB ports include their VID, PID and (when available) serial number in the
//! URI so that the URI can be used for stable device identification regardless
//! of the kernel-assigned port name.

use serialport::{available_ports, SerialPortType};

fn main() {
    let ports = match available_ports() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error enumerating serial ports: {}", e);
            std::process::exit(1);
        }
    };

    if ports.is_empty() {
        println!("No serial ports found.");
        return;
    }

    // Compute column width for the port-name column.
    let name_width = ports
        .iter()
        .map(|p| p.port_name.len())
        .max()
        .unwrap_or(0);

    for info in &ports {
        let uri = build_uri(info);
        println!("{:<width$}  {}", info.port_name, uri, width = name_width);
    }
}

/// Build the serial URI for a port.
///
/// For USB ports the VID, PID and (when available) serial number are included
/// so the URI can be used to identify the device by hardware rather than by
/// its kernel-assigned name.
fn build_uri(info: &serialport::SerialPortInfo) -> String {
    let mut params: Vec<String> = Vec::new();

    if let SerialPortType::UsbPort(usb) = &info.port_type {
        params.push(format!("vid=0x{:04x}", usb.vid));
        params.push(format!("pid=0x{:04x}", usb.pid));
        if let Some(serial) = &usb.serial_number {
            if !serial.is_empty() {
                params.push(format!("serial={}", serial));
            }
        }
    }

    let mut uri = format!("serial://{}", info.port_name);
    if !params.is_empty() {
        uri.push('?');
        uri.push_str(&params.join("&"));
    }
    uri
}
