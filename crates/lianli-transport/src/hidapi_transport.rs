use crate::error::TransportError;
use crate::hid_trait::HidTransport;
use hidapi::{HidApi, HidDevice};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::warn;

/// Which HID interface to open: hidapi lists every interface of a device separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidTarget {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: Option<u16>,
    pub serial: Option<String>,
}

impl HidTarget {
    pub fn open(&self) -> Result<HidDevice, TransportError> {
        let api = HidApi::new().map_err(|e| TransportError::Other(format!("hidapi: {e}")))?;
        let info = api
            .device_list()
            .find(|info| {
                info.vendor_id() == self.vid
                    && info.product_id() == self.pid
                    && self.usage_page.is_none_or(|page| info.usage_page() == page)
                    && self
                        .serial
                        .as_deref()
                        .is_none_or(|s| info.serial_number() == Some(s))
            })
            .ok_or(TransportError::DeviceNotFound {
                vid: self.vid,
                pid: self.pid,
            })?;
        info.open_device(&api).map_err(|e| {
            TransportError::Other(format!("opening {:04x}:{:04x}: {e}", self.vid, self.pid))
        })
    }
}

/// HID through the operating system's own HID driver (hidapi): what Windows needs, since
/// libusb can't open devices Windows' HID driver owns. A failed transfer reopens the device
/// once, like the hidraw transport.
pub struct HidApiTransport {
    device: HidDevice,
    target: HidTarget,
    reopens: AtomicU64,
}

impl HidApiTransport {
    pub fn open(target: HidTarget) -> Result<Self, TransportError> {
        Ok(Self {
            device: target.open()?,
            target,
            reopens: AtomicU64::new(0),
        })
    }

    fn with_reopen<T>(
        &mut self,
        label: &str,
        mut op: impl FnMut(&HidDevice) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        match op(&self.device) {
            Ok(v) => Ok(v),
            Err(e) if crate::usb::shutting_down() => Err(e),
            Err(e) => {
                warn!(
                    "HID {label} failed ({e}); reopening {:04x}:{:04x}",
                    self.target.vid, self.target.pid
                );
                self.device = self.target.open()?;
                self.reopens.fetch_add(1, Ordering::Relaxed);
                op(&self.device)
            }
        }
    }
}

fn write_error(e: hidapi::HidError) -> TransportError {
    TransportError::Write(e.to_string())
}

fn read_error(e: hidapi::HidError) -> TransportError {
    TransportError::Read(e.to_string())
}

impl HidTransport for HidApiTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize, TransportError> {
        self.with_reopen("write", |d| d.write(data).map_err(write_error))
    }

    /// Windows' HID writes already give up on their own (after about a second), so a
    /// bounded write is a single attempt without a reopen.
    fn write_timeout(&mut self, data: &[u8], _timeout: Duration) -> Result<usize, TransportError> {
        self.device.write(data).map_err(write_error)
    }

    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError> {
        self.with_reopen("read", |d| {
            d.read_timeout(buf, timeout_ms).map_err(read_error)
        })
    }

    fn send_feature_report(&mut self, data: &[u8]) -> Result<usize, TransportError> {
        self.with_reopen("feature report", |d| {
            d.send_feature_report(data)
                .map(|()| data.len())
                .map_err(write_error)
        })
    }

    fn get_feature_report(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        self.with_reopen("feature read", |d| {
            d.get_feature_report(buf).map_err(read_error)
        })
    }

    fn get_input_report(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        self.with_reopen("input report", |d| {
            d.get_input_report(buf).map_err(read_error)
        })
    }

    fn read_flush(&mut self) {
        let mut scratch = [0u8; 1024];
        // drain what's queued, bounded so a chatty device can't hold this forever
        for _ in 0..64 {
            match self.device.read_timeout(&mut scratch, 0) {
                Ok(n) if n > 0 => {}
                _ => break,
            }
        }
    }

    fn reopen_count(&self) -> u64 {
        self.reopens.load(Ordering::Relaxed)
    }
}
