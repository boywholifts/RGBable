//! Read-only device probe: lists the Lian Li hardware the drivers recognise.
//!
//! Nothing is opened for writing and no commands are sent, so it is safe to
//! run while L-Connect is still running (devices it holds just show fewer details).

use lianli_devices::detect::enumerate_devices;

const WIRELESS_TX: [(u16, u16); 2] = [(0x0416, 0x8040), (0x1A86, 0xE304)];
const WIRELESS_RX: [(u16, u16); 2] = [(0x0416, 0x8041), (0x1A86, 0xE305)];

fn main() -> anyhow::Result<()> {
    println!("lianli-probe (read-only), libusb {:?}", rusb::version());

    let detected = enumerate_devices()?;
    println!("\nRecognised wired devices: {}", detected.len());
    for d in &detected {
        println!(
            "  {:04x}:{:04x}  {:<32} family={:?}  bus={} addr={}  serial={}",
            d.vid,
            d.pid,
            d.name,
            d.family,
            d.bus,
            d.address,
            d.serial.as_deref().unwrap_or("-"),
        );
    }

    println!("\nWireless dongle:");
    let mut any = false;
    for device in lianli_transport::usb_context::devices()?.iter() {
        let Ok(desc) = device.device_descriptor() else { continue };
        let id = (desc.vendor_id(), desc.product_id());
        let role = if WIRELESS_TX.contains(&id) {
            "TX"
        } else if WIRELESS_RX.contains(&id) {
            "RX"
        } else {
            continue;
        };
        any = true;
        println!(
            "  {role} {:04x}:{:04x}  bus={} addr={} ports={:?}",
            id.0,
            id.1,
            device.bus_number(),
            device.address(),
            device.port_numbers().unwrap_or_default()
        );
    }
    if !any {
        println!("  none found");
    }
    Ok(())
}
