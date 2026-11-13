//! 通用 USB 枚举观测端：不认领接口，不发送存储命令，不修改 U 盘内容。

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Result, anyhow};
use esp_idf_svc::sys::{self, usb_hid as usb};

#[path = "discovery/describe.rs"]
mod describe;
#[path = "discovery/port.rs"]
mod port;

static SCAN_REQUESTED: AtomicBool = AtomicBool::new(true);
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

pub(super) fn start() -> Result<()> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("usb_observe".into())
        .stack_size(8192)
        .spawn(move || run(tx))?;
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(sys::ESP_OK) => Ok(()),
        Ok(error) => Err(anyhow!("USB observer registration failed: {error}")),
        Err(error) => Err(anyhow!("USB observer not ready: {error}")),
    }
}

pub(super) fn status() {
    library_info();
    port::status();
    SCAN_REQUESTED.store(true, Ordering::Relaxed);
}

pub(super) fn restart() {
    RESTART_REQUESTED.store(true, Ordering::Relaxed);
}

fn library_info() {
    let mut info = usb::usb_host_lib_info_t::default();
    let error = unsafe { usb::usb_host_lib_info(&mut info) };
    if error == sys::ESP_OK {
        log::info!(
            "USB Host: devices={} clients={} root_suspended={}",
            info.num_devices,
            info.num_clients,
            info.root_port_suspended
        );
    } else {
        log::warn!("usb_host_lib_info failed: {error}");
    }
}

fn run(ready: mpsc::Sender<i32>) {
    let mut config = usb::usb_host_client_config_t {
        is_synchronous: false,
        max_num_event_msg: 8,
        ..Default::default()
    };
    config.flags.set_notify_dev_removed(1);
    config.__bindgen_anon_1.async_ = usb::usb_host_client_config_t__bindgen_ty_2__bindgen_ty_1 {
        client_event_callback: Some(event),
        callback_arg: ptr::null_mut(),
    };
    let mut client = ptr::null_mut();
    let error = unsafe { usb::usb_host_client_register(&config, &mut client) };
    let _ = ready.send(error);
    if error != sys::ESP_OK {
        return;
    }
    log::info!("USB class-independent observer ready (read-only descriptors)");
    let mut seen = BTreeSet::new();
    loop {
        if RESTART_REQUESTED.swap(false, Ordering::Relaxed) {
            // 仅切换 Host 库的逻辑根端口电源，不操作板级 VBUS GPIO。
            let off = unsafe { usb::usb_host_lib_set_root_port_power(false) };
            thread::sleep(Duration::from_millis(200));
            let on = unsafe { usb::usb_host_lib_set_root_port_power(true) };
            log::info!(
                "USB logical root-port restart: off={off} on={on}; external VBUS is not measured"
            );
            port::status();
            SCAN_REQUESTED.store(true, Ordering::Relaxed);
        }
        if SCAN_REQUESTED.swap(false, Ordering::Relaxed) {
            scan(client, &mut seen);
        }
        let error = unsafe { usb::usb_host_client_handle_events(client, 50) };
        if error != sys::ESP_OK && error != sys::ESP_ERR_TIMEOUT {
            log::warn!("USB observer events failed: {error}");
        }
    }
}

unsafe extern "C" fn event(message: *const usb::usb_host_client_event_msg_t, _arg: *mut c_void) {
    if message.is_null() {
        return;
    }
    // 回调只触发重新扫描；阻塞式打开/描述符访问均在 handle_events 返回后执行。
    let kind = unsafe { (*message).event };
    if matches!(
        kind,
        usb::usb_host_client_event_t_USB_HOST_CLIENT_EVENT_NEW_DEV
            | usb::usb_host_client_event_t_USB_HOST_CLIENT_EVENT_DEV_REMOVED
            | usb::usb_host_client_event_t_USB_HOST_CLIENT_EVENT_DEV_GONE
    ) {
        SCAN_REQUESTED.store(true, Ordering::Relaxed);
    }
}

fn scan(client: usb::usb_host_client_handle_t, seen: &mut BTreeSet<u8>) {
    library_info();
    let mut addresses = [0u8; 16];
    let mut count = 0;
    let error = unsafe {
        usb::usb_host_device_addr_list_fill(
            addresses.len() as i32,
            addresses.as_mut_ptr(),
            &mut count,
        )
    };
    if error != sys::ESP_OK {
        log::warn!("USB address list failed: {error}");
        return;
    }
    let current: BTreeSet<_> = addresses[..(count.max(0) as usize).min(addresses.len())]
        .iter()
        .copied()
        .collect();
    for address in seen.difference(&current) {
        log::info!("USB device removed: address={address}");
    }
    for address in &current {
        describe::device(client, *address);
    }
    *seen = current;
}
