//! Minimal wireless control tool, used to test the Windows port on real hardware.
//!
//!   lianli_cli list
//!   lianli_cli color <index|mac> <r,g,b>
//!   lianli_cli fan   <index|mac> <percent> [hold_seconds]
//!   lianli_cli locate                      (3-fan groups -> red / green / blue)
//!   lianli_cli oled-ring <r,g,b>           (HydroShift II OLED Curve 45-LED ring)
//!   lianli_cli oled-image [image-file]     (no file = built-in orientation test card)
//!   lianli_cli oled-card <portrait|landscape>  (quadrants: TL red, TR green, BL blue, BR yellow)
//!   lianli_cli oled-ruler <portrait|landscape> (8 colour stripes along the long side)
//!   lianli_cli oled-ruler-png <portrait|landscape> <out.png>   (save instead of sending)
//!   lianli_cli oled-video <file.h264> [seconds=20] [fps=30]    (loop an H.264 stream)
//!   lianli_cli oled-play <image|gif|video> [--seconds N] [--fps N] [--fit cover|contain] [--rotate 90]
//!       Fits the file to the 2288x1080 landscape screen, rotates it into the
//!       panel's 1080x2288 frame, encodes it with ffmpeg and loops it. Runs until
//!       the window is closed unless --seconds is given. Needs ffmpeg on PATH.
//!
//! `color` uploads one static frame (the same upload path the daemon uses); the
//! device keeps showing it afterwards. `fan` re-sends the duty twice a second for
//! the hold time (default 15 s), because wireless fans fall back to their
//! hardware default when PWM traffic stops. L-Connect must be fully closed.

use anyhow::{bail, Context, Result};
use lianli_devices::wireless::{DiscoveredDevice, WirelessController};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,lianli_devices=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("list");

    match command {
        "oled-ring" => {
            let rgb = parse_rgb(args.get(1).context("missing colour, e.g. 128,0,255")?)?;
            return oled_ring(rgb);
        }
        "oled-image" => return oled_image(args.get(1).map(String::as_str)),
        "oled-card" => {
            let landscape = match args.get(1).map(String::as_str) {
                Some("landscape") => true,
                Some("portrait") | None => false,
                Some(other) => bail!("oled-card takes portrait or landscape, not `{other}`"),
            };
            return oled_card(landscape);
        }
        "oled-ruler" => {
            let landscape = match args.get(1).map(String::as_str) {
                Some("landscape") => true,
                Some("portrait") | None => false,
                Some(other) => bail!("oled-ruler takes portrait or landscape, not `{other}`"),
            };
            return oled_ruler(landscape);
        }
        "oled-ruler-png" => {
            let landscape = args.get(1).map(String::as_str) == Some("landscape");
            let out = args.get(2).context("missing output path")?;
            ruler_image(landscape)
                .save(out)
                .with_context(|| format!("writing {out}"))?;
            println!("Saved ruler to {out}");
            return Ok(());
        }
        "oled-play" => return oled_play(&args[1..]),
        "oled-video" => {
            let file = args.get(1).context("missing .h264 file")?;
            let seconds: u64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(20);
            let fps: f32 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(30.0);
            return oled_video(file, Some(seconds), fps);
        }
        _ => {}
    }

    let mut wireless = WirelessController::new();
    wireless
        .connect()
        .context("connecting to the wireless dongle")?;
    wireless
        .start_polling()
        .context("starting wireless discovery")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while wireless.discovered_device_count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let devices = wireless.devices();
    let result = match command {
        "locate" => locate(&wireless, &devices),
        "list" => {
            print_devices(&devices);
            Ok(())
        }
        "color" => {
            let device = pick(&devices, args.get(1))?;
            let rgb = parse_rgb(args.get(2).context("missing colour, e.g. 255,0,128")?)?;
            set_color(&wireless, &device, rgb)
        }
        "fan" => {
            let device = pick(&devices, args.get(1))?;
            let percent: u8 = args.get(2).context("missing percent")?.parse()?;
            let hold: u64 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(15);
            set_fan(&wireless, &device, percent, hold)
        }
        other => bail!(
            "unknown command `{other}` (list | color | fan | locate | oled-ring | oled-image)"
        ),
    };
    wireless.stop();
    result
}

fn print_devices(devices: &[DiscoveredDevice]) {
    println!("{} wireless device(s):", devices.len());
    for (i, d) in devices.iter().enumerate() {
        println!(
            "  [{i}] {}  {:<10}  fans={}  rpm={:?}  pwm={:?}",
            d.mac_str(),
            format!("{:?}", d.fan_type),
            d.fan_count,
            d.fan_rpms,
            d.current_pwm
        );
    }
}

fn pick(devices: &[DiscoveredDevice], which: Option<&String>) -> Result<DiscoveredDevice> {
    let which = which.context("missing device index or MAC (see `list`)")?;
    if let Ok(index) = which.parse::<usize>() {
        return devices
            .get(index)
            .cloned()
            .context("no device at that index");
    }
    devices
        .iter()
        .find(|d| d.mac_str().eq_ignore_ascii_case(which))
        .cloned()
        .context("no device with that MAC")
}

fn parse_rgb(text: &str) -> Result<[u8; 3]> {
    let parts: Vec<u8> = text
        .split(',')
        .map(|p| p.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .context("colour must be r,g,b with values 0-255")?;
    match parts.as_slice() {
        [r, g, b] => Ok([*r, *g, *b]),
        _ => bail!("colour must be exactly three numbers"),
    }
}

fn set_color(wireless: &WirelessController, device: &DiscoveredDevice, rgb: [u8; 3]) -> Result<()> {
    let leds: usize = device
        .fan_type
        .rgb_zone_led_counts(device.fan_count)
        .iter()
        .sum();
    println!(
        "Setting {} ({:?}, {leds} LEDs) to {rgb:?}",
        device.mac_str(),
        device.fan_type
    );
    let upload = wireless.prepare_rgb_upload(&device.mac, &[vec![rgb; leds]], 5000)?;
    const ATTEMPTS: u32 = 3;
    for attempt in 1..=ATTEMPTS {
        wireless.send_rgb_upload(&device.mac, &upload, 4)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if wireless.rgb_upload_applied(&device.mac, upload.effect_index()) {
                println!("Device confirmed the new effect (attempt {attempt}).");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if attempt < ATTEMPTS {
            println!(
                "  no confirmation yet, re-sending (attempt {}/{ATTEMPTS})",
                attempt + 1
            );
        }
    }
    println!("Sent {ATTEMPTS} times, but the device never confirmed (check it visually).");
    Ok(())
}

fn set_fan(
    wireless: &WirelessController,
    device: &DiscoveredDevice,
    percent: u8,
    hold: u64,
) -> Result<()> {
    anyhow::ensure!(device.fan_count > 0, "that device has no fans");
    let percent = percent.min(100);
    let duty = ((u16::from(percent) * 255 + 50) / 100) as u8;
    let mut pwm = [0u8; 4];
    for slot in pwm.iter_mut().take(usize::from(device.fan_count)) {
        *slot = duty;
    }
    println!(
        "Holding {} at {percent}% (duty {duty}) for {hold} s",
        device.mac_str()
    );
    let end = Instant::now() + Duration::from_secs(hold);
    let mut last_print = Instant::now() - Duration::from_secs(10);
    while Instant::now() < end {
        wireless.set_fan_speeds_by_mac(&device.mac, &pwm)?;
        if last_print.elapsed() >= Duration::from_secs(2) {
            last_print = Instant::now();
            if let Some(now) = wireless.device_by_mac(&device.mac) {
                println!(
                    "  rpm={:?} reported pwm={:?}",
                    now.fan_rpms, now.current_pwm
                );
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    println!("Hold finished; fans return to their default until L-Connect takes over again.");
    Ok(())
}

fn locate(wireless: &WirelessController, devices: &[DiscoveredDevice]) -> Result<()> {
    const COLOURS: [(&str, [u8; 3]); 4] = [
        ("RED", [255, 0, 0]),
        ("GREEN", [0, 255, 0]),
        ("BLUE", [0, 0, 255]),
        ("YELLOW", [255, 200, 0]),
    ];
    let mut groups: Vec<&DiscoveredDevice> = devices.iter().filter(|d| d.fan_count >= 2).collect();
    groups.sort_by_key(|d| d.mac);
    for (device, (name, rgb)) in groups.iter().zip(COLOURS) {
        println!(
            "LOCATE {} ({} fans) = {name}",
            device.mac_str(),
            device.fan_count
        );
        set_color(wireless, device, rgb)?;
    }
    Ok(())
}

fn find_wired(pid: u16) -> Result<lianli_devices::detect::DetectedDevice> {
    lianli_devices::detect::enumerate_devices()?
        .into_iter()
        .find(|d| d.vid == if pid == 0x8051 { 0x0416 } else { 0x1CBE } && d.pid == pid)
        .with_context(|| format!("device with PID {pid:04x} not found"))
}

fn oled_ring(rgb: [u8; 3]) -> Result<()> {
    use lianli_devices::winusb::hs2_oled_led::Hs2OledLedController;
    let det = find_wired(0x8051)?;
    let led = Hs2OledLedController::new(det.device)
        .context("opening OLED LED MCU (is L-Connect closed?)")?;
    println!("Setting OLED ambient ring (45 LEDs) to {rgb:?}");
    led.send_rgb_frame(&[rgb; 45])?;
    println!("Sent.");
    Ok(())
}

/// Built-in card: Gengar purple -> red gradient with white orientation markers
/// (big square = top-left of the 1080x2288 frame, small square = top-right).
fn test_card(width: u32, height: u32) -> image::RgbImage {
    image::RgbImage::from_fn(width, height, |x, y| {
        let t = y as f32 / height as f32;
        let mut px = [
            (90.0 + 130.0 * t) as u8,
            (20.0 * (1.0 - t)) as u8,
            (200.0 * (1.0 - t) + 40.0 * t) as u8,
        ];
        let big = x < 240 && y < 240;
        let small = x >= width - 120 && y < 120;
        let border = x < 8 || y < 8 || x >= width - 8 || y >= height - 8;
        if big || small || border {
            px = [255, 255, 255];
        }
        image::Rgb(px)
    })
}

fn oled_image(path: Option<&str>) -> Result<()> {
    use std::io::Cursor;
    const W: u32 = 1080;
    const H: u32 = 2288;
    const MAX_PNG: usize = 1_048_576;

    let img = match path {
        None => test_card(W, H),
        Some(p) => {
            let src = image::open(p)
                .with_context(|| format!("opening {p}"))?
                .to_rgb8();
            // The panel is landscape; accept landscape art by rotating it into the portrait frame.
            let src = if src.width() > src.height() {
                image::imageops::rotate90(&src)
            } else {
                src
            };
            let resized =
                image::imageops::resize(&src, W, H, image::imageops::FilterType::Lanczos3);
            resized
        }
    };
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("encoding PNG")?;
    println!("PNG frame: {W}x{H}, {} KiB", png.len() / 1024);
    anyhow::ensure!(
        png.len() <= MAX_PNG,
        "PNG is {} KiB; the panel accepts at most 1024 KiB per frame (try a simpler image)",
        png.len() / 1024
    );

    send_oled_png(&png)
}

fn send_oled_png(png: &[u8]) -> Result<()> {
    use lianli_devices::winusb::lcd::WinUsbLcdDevice;
    let det = find_wired(0xA068)?;
    let mut lcd =
        WinUsbLcdDevice::open(det.device, 0xA068).context("opening OLED (is L-Connect closed?)")?;
    lcd.send_frame_verified(png).context("sending frame")?;
    println!("Frame shown on the OLED.");
    Ok(())
}

/// Four solid quadrants, so any rotation, mirroring or tiling is obvious:
/// top-left RED, top-right GREEN, bottom-left BLUE, bottom-right YELLOW,
/// with a thin white cross through the centre.
fn oled_card(landscape: bool) -> Result<()> {
    use std::io::Cursor;
    let (w, h) = if landscape {
        (2288u32, 1080u32)
    } else {
        (1080u32, 2288u32)
    };
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if x.abs_diff(w / 2) < 6 || y.abs_diff(h / 2) < 6 {
            return image::Rgb([255, 255, 255]);
        }
        image::Rgb(match (x < w / 2, y < h / 2) {
            (true, true) => [255, 0, 0],
            (false, true) => [0, 255, 0],
            (true, false) => [0, 0, 255],
            (false, false) => [255, 220, 0],
        })
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("encoding PNG")?;
    println!(
        "{} card {w}x{h}, {} KiB (TL red, TR green, BL blue, BR yellow)",
        if landscape { "Landscape" } else { "Portrait" },
        png.len() / 1024
    );
    send_oled_png(&png)
}

/// Ruler card: the 2288-pixel long side is cut into 8 stripes, in order
/// RED, ORANGE, YELLOW, GREEN, CYAN, BLUE, PURPLE, WHITE. Across the 1080-pixel
/// short side, the first half is full brightness and the second half is dimmed.
/// Which stripes appear, in what order and how often, shows how the panel maps
/// the frame.
fn oled_ruler(landscape: bool) -> Result<()> {
    use std::io::Cursor;
    let img = ruler_image(landscape);
    let (w, h) = img.dimensions();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("encoding PNG")?;
    println!(
        "{} ruler {w}x{h}, {} KiB",
        if landscape { "LANDSCAPE" } else { "PORTRAIT" },
        png.len() / 1024
    );
    send_oled_png(&png)
}

fn ruler_image(landscape: bool) -> image::RgbImage {
    const STRIPES: [[u8; 3]; 8] = [
        [255, 0, 0],
        [255, 128, 0],
        [255, 255, 0],
        [0, 255, 0],
        [0, 255, 255],
        [0, 0, 255],
        [160, 0, 255],
        [255, 255, 255],
    ];
    const LONG: u32 = 2288;
    const SHORT: u32 = 1080;
    let (w, h) = if landscape {
        (LONG, SHORT)
    } else {
        (SHORT, LONG)
    };
    image::RgbImage::from_fn(w, h, |x, y| {
        let (along, across) = if landscape { (x, y) } else { (y, x) };
        let stripe = (along * 8 / LONG).min(7) as usize;
        let mut px = STRIPES[stripe];
        if across >= SHORT / 2 {
            px = px.map(|c| c / 4);
        }
        image::Rgb(px)
    })
}

/// Loop an Annex-B H.264 file on the OLED for `seconds`, the way the daemon
/// plays media on this panel (it never uses the PNG path when H.264 is available).
fn oled_video(file: &str, seconds: Option<u64>, fps: f32) -> Result<()> {
    use lianli_devices::winusb::lcd::WinUsbLcdDevice;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let det = find_wired(0xA068)?;
    let mut lcd =
        WinUsbLcdDevice::open(det.device, 0xA068).context("opening OLED (is L-Connect closed?)")?;
    let stop = Arc::new(AtomicBool::new(false));
    let timer = seconds.map(|seconds| {
        let stop = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(seconds));
            stop.store(true, Ordering::Release);
        })
    });
    match seconds {
        Some(seconds) => println!("Streaming {file} at {fps} fps for {seconds} s..."),
        None => println!("Streaming {file} at {fps} fps until this window is closed..."),
    }
    let result = lcd.stream_h264(std::path::Path::new(file), true, &stop, fps);
    stop.store(true, Ordering::Release);
    if let Some(timer) = timer {
        let _ = timer.join();
    }
    result.context("streaming H.264")?;
    println!("Stream finished.");
    Ok(())
}

struct PlayOptions {
    file: String,
    seconds: Option<u64>,
    fps: u32,
    contain: bool,
    rotate: u32,
}

fn parse_play_options(args: &[String]) -> Result<PlayOptions> {
    let mut file = None;
    let mut options = PlayOptions {
        file: String::new(),
        seconds: None,
        fps: 30,
        contain: false,
        rotate: 90,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--seconds" => options.seconds = Some(value("--seconds")?.parse()?),
            "--fps" => options.fps = value("--fps")?.parse::<u32>()?.clamp(1, 60),
            "--fit" => {
                options.contain = match value("--fit")?.as_str() {
                    "contain" => true,
                    "cover" => false,
                    other => bail!("--fit takes cover or contain, not `{other}`"),
                }
            }
            "--rotate" => {
                options.rotate = value("--rotate")?.parse()?;
                anyhow::ensure!(
                    matches!(options.rotate, 0 | 90 | 180 | 270),
                    "--rotate takes 0, 90, 180 or 270"
                );
            }
            other if other.starts_with("--") => bail!("unknown option `{other}`"),
            other => file = Some(other.to_string()),
        }
    }
    options.file = file.context("missing image or video file")?;
    Ok(options)
}

/// ffmpeg filter: fit to the landscape screen, then rotate into the panel frame.
/// Rotation 90 (transpose=1) is the orientation confirmed on real hardware:
/// landscape content then appears upright on the curved OLED.
fn play_filter(options: &PlayOptions) -> String {
    const W: u32 = 2288;
    const H: u32 = 1080;
    let (w, h) = if options.rotate == 90 || options.rotate == 270 {
        (W, H)
    } else {
        (H, W)
    };
    let fit = if options.contain {
        format!("scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black")
    } else {
        format!("scale={w}:{h}:force_original_aspect_ratio=increase:flags=lanczos,crop={w}:{h}")
    };
    let rotate = match options.rotate {
        90 => ",transpose=1",
        180 => ",transpose=1,transpose=1",
        270 => ",transpose=2",
        _ => "",
    };
    format!("{fit}{rotate},fps={}", options.fps)
}

fn oled_play(args: &[String]) -> Result<()> {
    let options = parse_play_options(args)?;
    anyhow::ensure!(
        std::path::Path::new(&options.file).is_file(),
        "{} does not exist",
        options.file
    );
    let lower = options.file.to_lowercase();
    let is_still = [".png", ".jpg", ".jpeg", ".bmp", ".webp", ".tif", ".tiff"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    let output = std::env::temp_dir().join("lianli-oled-play.h264");

    let mut ffmpeg = std::process::Command::new("ffmpeg");
    ffmpeg.args(["-y", "-loglevel", "error"]);
    if is_still {
        ffmpeg.args(["-loop", "1", "-t", "1"]);
    }
    ffmpeg
        .arg("-i")
        .arg(&options.file)
        .args(["-vf", &play_filter(&options)])
        .args([
            "-r",
            &options.fps.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .args([
            "-x264opts",
            "bframes=0",
            "-threads",
            "4",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(["-color_range", "pc", "-an", "-f", "h264"])
        .arg(&output);
    println!(
        "Encoding {} ({}, rotate {}, {} fps)...",
        options.file,
        if options.contain { "contain" } else { "cover" },
        options.rotate,
        options.fps
    );
    let status = ffmpeg
        .status()
        .context("running ffmpeg (is it installed? winget install Gyan.FFmpeg)")?;
    anyhow::ensure!(status.success(), "ffmpeg failed with {status}");
    let size = std::fs::metadata(&output)?.len();
    println!("Encoded {} KiB of H.264.", size / 1024);

    oled_video(
        &output.to_string_lossy(),
        options.seconds,
        options.fps as f32,
    )
}
