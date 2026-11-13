//! 独立 USB 线路诊断：不启动 BLE/HID，不改 GPIO 方向、上下拉或板级供电。

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use esp_idf_svc::sys::{self, usb_hid as usb};

#[path = "usb/discovery.rs"]
#[allow(dead_code)] // 独立程序不提供主固件的串口根端口重启命令。
mod discovery;

fn main() -> Result<()> {
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("USB GPIO CHECK: standalone; BLE/HID disabled; no storage access");

    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("usb_library".into())
        .stack_size(8192)
        .spawn(move || {
            let config = usb::usb_host_config_t {
                skip_phy_setup: false,
                root_port_unpowered: false,
                intr_flags: sys::ESP_INTR_FLAG_LOWMED as _,
                ..Default::default()
            };
            let error = unsafe { usb::usb_host_install(&config) };
            let _ = tx.send(error);
            if error != sys::ESP_OK {
                return;
            }
            loop {
                let mut flags = 0;
                let error = unsafe { usb::usb_host_lib_handle_events(u32::MAX, &mut flags) };
                if error != sys::ESP_OK {
                    log::warn!("USB Host event error={error}");
                    thread::sleep(Duration::from_millis(10));
                }
            }
        })?;
    let error = rx.recv_timeout(Duration::from_secs(5))?;
    if error != sys::ESP_OK {
        return Err(anyhow!("USB Host install failed: {error}"));
    }
    discovery::start()?;
    log::info!(
        "GPIO19=D- GPIO20=D+; gpio_get_level is read-only but may be invalid with USB PHY enabled; use HPRT line_state for USB detection"
    );

    let mut previous = None;
    let mut heartbeat = Instant::now();
    loop {
        // 不调用 gpio_config/set_direction：保留 USB PHY 对引脚的控制。
        let snapshot = unsafe {
            (
                sys::gpio_get_level(19),
                sys::gpio_get_level(20),
                std::ptr::addr_of!(usb::USB_DWC.hprt_reg.val).read_volatile(),
            )
        };
        if previous != Some(snapshot) || heartbeat.elapsed() >= Duration::from_secs(5) {
            log::info!(
                "USB GPIO CHECK: raw_gpio19={} raw_gpio20={} (not authoritative) HPRT={:#010x}",
                snapshot.0,
                snapshot.1,
                snapshot.2
            );
            discovery::status();
            previous = Some(snapshot);
            heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(20));
    }
}
