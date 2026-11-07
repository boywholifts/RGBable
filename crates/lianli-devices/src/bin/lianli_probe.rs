//! Read-only device probe: lists the Lian Li hardware the drivers recognise.
//!
//! Default mode opens nothing and sends no commands, so it is safe to run while
//! L-Connect is still running (devices it holds just show fewer details).
//!
//! `--live` additionally opens the wireless RX dongle and the HydroShift II
//! OLED Curve LED MCU and sends only *query* commands (GetDev, GetVer, GetTemp,
//! GetPump). No colours, speeds, motor moves, binds or channel changes are sent.
//! L-Connect must be fully closed first.

use lianli_devices::detect::enumerate_devices;

const WIRELESS_TX: [(u16, u16); 2] = [(0x0416, 0x8040), (0x1A86, 0xE304)];
const WIRELESS_RX: [(u16, u16); 2] = [(0x0416, 0x8041), (0x1A86, 0xE305)];

fn main() -> anyhow::Result<()> {
    let live = std::env::args().any(|arg| arg == "--live");
    println!(
        "lianli-probe ({}), libusb {:?}",
        if live { "live read-only queries" } else { "enumerate only" },
        rusb::version()
    );

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

    if live {
        live_wireless();
        live_hs2_led(&detected);
    }
    Ok(())
}

fn live_wireless() {
    use lianli_devices::wireless::read_only_snapshot;
    println!("\n== Wireless device list (GetDev via RX)");
    match read_only_snapshot() {
        Err(e) => println!("  FAILED: {e:#}"),
        Ok(records) => {
            println!("  {} record(s)", records.len());
            for (i, r) in records.iter().enumerate() {
                if let Some((mac, ch)) = r.master {
                    println!("  [{i}] master dongle {mac:02x?} channel={ch}");
                } else if let Some(d) = &r.device {
                    println!(
                        "  [{i}] {}  type={:?}  device_type={}  fans={}  fan_type_bytes={:?}",
                        d.mac_str(), d.fan_type, d.device_type, d.fan_count, d.fan_types
                    );
                    println!(
                        "       rpm={:?} pwm={:?} master={:02x?} ch={} coolant={:?} mb_sync_light={} pwm_line={}",
                        d.fan_rpms, d.current_pwm, d.master_mac, d.channel,
                        d.coolant_temp_c, d.is_sync_mb_light, d.is_pwm_line_on
                    );
                } else {
                    println!("  [{i}] unparsed record");
                }
                println!("       raw={}", hex(&r.raw));
            }
        }
    }
}

fn live_hs2_led(detected: &[lianli_devices::detect::DetectedDevice]) {
    use lianli_devices::winusb::hs2_oled_led::Hs2OledLedController;
    println!("\n== HydroShift II OLED Curve LED MCU (0416:8051)");
    let Some(det) = detected.iter().find(|d| d.vid == 0x0416 && d.pid == 0x8051) else {
        println!("  not detected");
        return;
    };
    let led = match Hs2OledLedController::new(det.device.clone()) {
        Ok(led) => led,
        Err(e) => {
            println!("  open FAILED: {e:#}  (is L-Connect fully closed?)");
            return;
        }
    };
    match led.read_firmware() {
        Ok(v) => println!("  firmware: {v}"),
        Err(e) => println!("  firmware FAILED: {e:#}"),
    }
    match led.read_temps() {
        Ok((coolant, flag)) => println!("  coolant: {coolant} C  (mb-sync flag byte {flag})"),
        Err(e) => println!("  temps FAILED: {e:#}"),
    }
    match led.read_pump_rpm_raw() {
        Ok(rpm) => println!("  pump: {rpm} RPM"),
        Err(e) => println!("  pump FAILED: {e:#}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}
