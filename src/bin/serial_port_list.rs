//! `serial-port-list` — List all available serial ports and their URIs.
//!
//! # Usage
//!
//! ```text
//! serial-port-list            # list available ports
//! serial-port-list --format   # show URI format and parameter reference
//! ```
//!
//! # Example output
//!
//! USB ports with complete identifiers (vid, pid, serial) are listed first and
//! display two URI forms — one by port name and one by USB identifiers:
//!
//! ```text
//! # /dev/ttyUSB0
//! - serial:///dev/ttyUSB0
//! - serial://?vid=0x2341&pid=0x0043&serial=DFSZGL
//!
//! # /dev/ttyS0
//! - serial:///dev/ttyS0
//! ```

use serialport::{available_ports, SerialPortType};
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--format") {
        print_format_help();
        return;
    }

    let mut ports = match available_ports() {
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

    // Sort: USB ports with complete identifiers first, then other USB ports,
    // then non-USB ports.
    ports.sort_by_key(|p| match &p.port_type {
        SerialPortType::UsbPort(usb) => {
            if has_complete_usb_info(usb) {
                0
            } else {
                1
            }
        }
        _ => 2,
    });

    for info in &ports {
        println!("# {}", info.port_name);
        println!("- serial://{}", info.port_name);

        if let SerialPortType::UsbPort(usb) = &info.port_type {
            if has_complete_usb_info(usb) {
                println!(
                    "- serial://?vid=0x{:04x}&pid=0x{:04x}&serial={}",
                    usb.vid,
                    usb.pid,
                    usb.serial_number.as_deref().unwrap_or("")
                );
            }
        }

        println!();
    }
}

/// Return `true` when the USB port has vid, pid **and** a non-empty serial
/// number — the three fields needed for a complete USB-identifier URI.
fn has_complete_usb_info(usb: &serialport::UsbPortInfo) -> bool {
    // vid and pid are always present on UsbPortInfo (they are u16, not Option),
    // so we only need to check the serial number.
    matches!(&usb.serial_number, Some(s) if !s.is_empty())
}

/// Print a reference of the serial URI format and all supported query
/// parameters with their possible values.
fn print_format_help() {
    println!("Serial URI format");
    println!("=================");
    println!();
    println!("Two forms are supported:");
    println!();
    println!("  Port-name form:");
    println!("    serial://<port>?<params>");
    println!("    e.g.  serial:///dev/ttyUSB0?baud=9600&parity=n");
    println!("    e.g.  serial://COM3?baud=115200");
    println!();
    println!("  USB VID/PID form (preferred — stable across reboots):");
    println!("    serial://?vid=<hex>&pid=<hex>&serial=<string>");
    println!("    e.g.  serial://?vid=0x2341&pid=0x0043&serial=DFSZGL");
    println!();
    println!("Query parameters");
    println!("----------------");
    println!();
    println!("  {:<10} {:<45} Default", "Key", "Values");
    println!("  {:<10} {:<45} -------", "---", "------");
    println!(
        "  {:<10} {:<45} 9600",
        "baud", "integer baud rate (e.g. 9600, 115200)"
    );
    println!(
        "  {:<10} {:<45} n",
        "parity", "n | none, e | even, o | odd"
    );
    println!("  {:<10} {:<45} 8", "data", "5, 6, 7, 8");
    println!("  {:<10} {:<45} 1", "stop", "1, 2");
    println!(
        "  {:<10} {:<45} none",
        "flow", "none, hw | hardware, sw | software"
    );
    println!(
        "  {:<10} {:<45} -",
        "vid", "USB vendor ID in hex (e.g. 0x2341)"
    );
    println!(
        "  {:<10} {:<45} -",
        "pid", "USB product ID in hex (e.g. 0x0043)"
    );
    println!("  {:<10} {:<45} -", "serial", "USB serial-number string");
}
