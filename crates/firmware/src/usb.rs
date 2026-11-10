//! USB Host HID input.
//!
//! ESP32-S3 acts as a USB host for one or more 2.4 GHz receiver interfaces.
//! The `espressif/usb_host_hid` managed component enumerates the device and
//! delivers raw input reports; this module owns the C driver lifecycle and
//! turns reports into [`UsbEvent`] values for the main loop.

#[cfg(not(esp_idf_comp_espressif__usb_host_hid_enabled))]
compile_error!(
    "the espressif/usb_host_hid component was not enabled; check the \
     [[package.metadata.esp-idf-sys.extra_components]] entry and re-run the build"
);

use std::ffi::c_void;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use bt_switch_core::MAX_SOURCES;
use bt_switch_core::hid::{Decoded, MAX_REPORTS, Parser};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys;
use esp_idf_svc::sys::usb_hid as usb;

/// Decoded input from one HID interface, or a detach notification.
pub enum UsbEvent {
    Input { source: u8, state: Decoded },
    Detached { source: u8 },
}

struct InterfaceCtx {
    handle: usize,
    source: u8,
    parser: Parser,
    states: [Decoded; MAX_REPORTS],
    invalid_reports: u32,
}

static TX: OnceLock<SyncSender<UsbEvent>> = OnceLock::new();
static INTERFACES: OnceLock<Mutex<Vec<InterfaceCtx>>> = OnceLock::new();
static OVERFLOW: AtomicBool = AtomicBool::new(false);
static ATTACHED: AtomicUsize = AtomicUsize::new(0);
static REPORTS: AtomicUsize = AtomicUsize::new(0);

pub struct UsbInput {
    pub events: Receiver<UsbEvent>,
}

impl UsbInput {
    /// Returns true once if input events had to be dropped.
    pub fn take_overflow(&self) -> bool {
        OVERFLOW.swap(false, Ordering::Relaxed)
    }

    /// True while at least one HID interface is open.
    pub fn attached(&self) -> bool {
        ATTACHED.load(Ordering::Relaxed) > 0
    }

    pub fn status(&self) {
        log::info!(
            "USB HID: {} interfaces, {} reports decoded",
            ATTACHED.load(Ordering::Relaxed),
            REPORTS.load(Ordering::Relaxed)
        );
    }
}

/// Install the USB host library and the HID class driver.
pub fn start() -> Result<UsbInput> {
    let (tx, rx) = sync_channel(128);
    TX.set(tx)
        .map_err(|_| anyhow!("USB input already started"))?;
    INTERFACES
        .set(Mutex::new(Vec::new()))
        .map_err(|_| anyhow!("USB input already started"))?;

    // The ESP-IDF USB Host library needs a task that handles library-level
    // events (enumeration, client registration and device freeing).
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("usb_lib".into())
        .stack_size(8192)
        .spawn(move || usb_library_task(ready_tx))?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(err) if err == sys::ESP_OK => {}
        Ok(err) => bail!("usb_host_install failed: {err}"),
        Err(err) => bail!("USB library task did not start: {err}"),
    }

    let config = usb::hid_host_driver_config_t {
        create_background_task: true,
        task_priority: 5,
        stack_size: 8192,
        core_id: i32::MAX, // tskNO_AFFINITY
        callback: Some(driver_callback),
        callback_arg: ptr::null_mut(),
    };
    let err = unsafe { usb::hid_host_install(&config) };
    if err != sys::ESP_OK {
        bail!("hid_host_install failed: {err}");
    }

    log::info!("USB host HID driver installed");
    Ok(UsbInput { events: rx })
}

fn usb_library_task(ready: std::sync::mpsc::Sender<i32>) {
    let mut config: usb::usb_host_config_t = unsafe { mem::zeroed() };
    config.skip_phy_setup = false;
    config.root_port_unpowered = false;
    config.intr_flags = sys::ESP_INTR_FLAG_LOWMED as _;

    let err = unsafe { usb::usb_host_install(&config) };
    let _ = ready.send(err);
    if err != sys::ESP_OK {
        return;
    }

    loop {
        let mut flags = 0u32;
        let err = unsafe { usb::usb_host_lib_handle_events(u32::MAX, &mut flags) };
        if err != sys::ESP_OK {
            log::warn!("usb_host_lib_handle_events: {err}");
            FreeRtos::delay_ms(10);
        }
        if flags & usb::USB_HOST_LIB_EVENT_FLAGS_NO_CLIENTS != 0 {
            let _ = unsafe { usb::usb_host_device_free_all() };
        }
    }
}

unsafe extern "C" fn driver_callback(
    handle: usb::hid_host_device_handle_t,
    event: usb::hid_host_driver_event_t,
    _arg: *mut c_void,
) {
    if event != usb::hid_host_driver_event_t_HID_HOST_DRIVER_EVENT_CONNECTED || handle.is_null() {
        return;
    }

    let mut params: usb::hid_host_dev_params_t = unsafe { mem::zeroed() };
    let err = unsafe { usb::hid_host_device_get_params(handle, &mut params) };
    if err != sys::ESP_OK {
        log::warn!("hid_host_device_get_params: {err}");
        return;
    }

    let config = usb::hid_host_device_config_t {
        callback: Some(interface_callback),
        callback_arg: ptr::null_mut(),
    };
    let err = unsafe { usb::hid_host_device_open(handle, &config) };
    if err != sys::ESP_OK {
        log::warn!("hid_host_device_open: {err}");
        return;
    }

    let mut descriptor_len = 0usize;
    let descriptor = unsafe { usb::hid_host_get_report_descriptor(handle, &mut descriptor_len) };
    let parser = if descriptor.is_null() || descriptor_len == 0 {
        None
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(descriptor, descriptor_len) };
        match Parser::parse(bytes) {
            Ok(parser) => Some(parser),
            Err(err) => {
                log::warn!(
                    "unsupported HID report descriptor ({} bytes): {err}",
                    descriptor_len
                );
                None
            }
        }
    };
    let Some(parser) = parser else {
        unsafe { usb::hid_host_device_close(handle) };
        return;
    };

    let Some(source) = allocate_source() else {
        log::warn!("too many HID interfaces (max {MAX_SOURCES})");
        unsafe { usb::hid_host_device_close(handle) };
        return;
    };

    {
        let mut interfaces = lock_interfaces();
        interfaces.push(InterfaceCtx {
            handle: handle as usize,
            source,
            parser,
            states: [Decoded::default(); MAX_REPORTS],
            invalid_reports: 0,
        });
    }

    // Report protocol preserves wheel, extra buttons, NKRO and consumer keys.
    // Never assume the receiver emits an 8-byte boot report.
    if params.sub_class == 1 {
        let err = unsafe {
            usb::hid_class_request_set_protocol(
                handle,
                usb::hid_report_protocol_t_HID_REPORT_PROTOCOL_REPORT,
            )
        };
        if err != sys::ESP_OK {
            log::warn!(
                "SET_PROTOCOL(report) on interface {}: {err}",
                params.iface_num
            );
        }
    }

    let mut info: usb::hid_host_dev_info_t = unsafe { mem::zeroed() };
    if unsafe { usb::hid_host_get_device_info(handle, &mut info) } == sys::ESP_OK {
        log::info!(
            "USB HID connected VID={:04x} PID={:04x} iface={} proto={} source={}",
            info.VID,
            info.PID,
            params.iface_num,
            params.proto,
            source
        );
    }

    let err = unsafe { usb::hid_host_device_start(handle) };
    if err != sys::ESP_OK {
        log::warn!("hid_host_device_start: {err}");
        remove_interface(handle as usize);
        unsafe { usb::hid_host_device_close(handle) };
        return;
    }

    ATTACHED.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "C" fn interface_callback(
    handle: usb::hid_host_device_handle_t,
    event: usb::hid_host_interface_event_t,
    _arg: *mut c_void,
) {
    if handle.is_null() {
        return;
    }

    match event {
        usb::hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_INPUT_REPORT => {
            let mut data = [0u8; 256];
            let mut length = 0usize;
            let err = unsafe {
                usb::hid_host_device_get_raw_input_report_data(
                    handle,
                    data.as_mut_ptr(),
                    data.len(),
                    &mut length,
                )
            };
            if err != sys::ESP_OK {
                return;
            }
            let length = length.min(data.len());
            let packet = &data[..length];

            let payload = {
                let mut interfaces = lock_interfaces();
                let Some(interface) = interfaces
                    .iter_mut()
                    .find(|interface| interface.handle == handle as usize)
                else {
                    return;
                };
                let InterfaceCtx {
                    parser,
                    states,
                    source,
                    invalid_reports,
                    ..
                } = interface;

                let Some((decoded, index)) = parser.decode(packet) else {
                    *invalid_reports = invalid_reports.saturating_add(1);
                    if *invalid_reports <= 3 {
                        log::warn!(
                            "HID interface {}: unknown or truncated report ({} bytes)",
                            *source,
                            length
                        );
                    }
                    return;
                };

                // Keep per-report-ID state and merge it, like the C prototype.
                // Relative motion belongs only to this packet.
                states[index] = decoded.without_motion();
                let mut merged = Decoded {
                    x: decoded.x,
                    y: decoded.y,
                    wheel: decoded.wheel,
                    pan: decoded.pan,
                    ..Default::default()
                };
                for state in &states[..parser.report_count()] {
                    merged.merge_state(state);
                }
                UsbEvent::Input {
                    source: *source,
                    state: merged,
                }
            };

            REPORTS.fetch_add(1, Ordering::Relaxed);
            send(payload);
        }
        usb::hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_TRANSFER_ERROR => {
            log::warn!("HID interface transfer error; releasing held input");
            let payload = {
                let mut interfaces = lock_interfaces();
                let Some(interface) = interfaces
                    .iter_mut()
                    .find(|interface| interface.handle == handle as usize)
                else {
                    return;
                };
                interface.states = [Decoded::default(); MAX_REPORTS];
                UsbEvent::Input {
                    source: interface.source,
                    state: Decoded::default(),
                }
            };
            send(payload);
        }
        usb::hid_host_interface_event_t_HID_HOST_INTERFACE_EVENT_DISCONNECTED => {
            let removed = {
                let mut interfaces = lock_interfaces();
                interfaces
                    .iter()
                    .position(|interface| interface.handle == handle as usize)
                    .map(|index| interfaces.swap_remove(index))
            };
            if let Some(interface) = removed {
                if ATTACHED.load(Ordering::Relaxed) > 0 {
                    ATTACHED.fetch_sub(1, Ordering::Relaxed);
                }
                log::info!("USB HID interface {} disconnected", interface.source);
                send(UsbEvent::Detached {
                    source: interface.source,
                });
            }
            let err = unsafe { usb::hid_host_device_close(handle) };
            if err != sys::ESP_OK {
                log::warn!("hid_host_device_close: {err}");
            }
        }
        other => {
            log::debug!("HID interface event {other:?} ignored");
        }
    }
}

fn allocate_source() -> Option<u8> {
    let interfaces = lock_interfaces();
    (0..MAX_SOURCES as u8).find(|source| {
        !interfaces
            .iter()
            .any(|interface| interface.source == *source)
    })
}

fn remove_interface(handle: usize) {
    let mut interfaces = lock_interfaces();
    interfaces.retain(|interface| interface.handle != handle);
}

fn send(event: UsbEvent) {
    if let Some(tx) = TX.get()
        && let Err(TrySendError::Full(_)) = tx.try_send(event)
    {
        OVERFLOW.store(true, Ordering::Relaxed);
    }
}

fn lock_interfaces() -> MutexGuard<'static, Vec<InterfaceCtx>> {
    INTERFACES
        .get()
        .expect("USB input not started")
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
