//! 从 Host 库读取缓存描述符；打开后必定关闭，不占用 HID 或存储接口。

use std::ptr;

use bt_switch_core::usb_descriptor::{self, Descriptors, DeviceIdentity, class_name};
use esp_idf_svc::sys::{self, usb_hid as usb};

pub(super) fn device(client: usb::usb_host_client_handle_t, address: u8) {
    let mut handle = ptr::null_mut();
    let error = unsafe { usb::usb_host_device_open(client, address, &mut handle) };
    if error != sys::ESP_OK {
        log::warn!("USB address={address} open failed: {error}");
        return;
    }
    describe(handle, address);
    let error = unsafe { usb::usb_host_device_close(client, handle) };
    if error != sys::ESP_OK {
        log::warn!("USB address={address} close failed: {error}");
    }
}

fn describe(handle: usb::usb_device_handle_t, address: u8) {
    let mut descriptor = ptr::null();
    let error = unsafe { usb::usb_host_get_device_descriptor(handle, &mut descriptor) };
    if error != sys::ESP_OK || descriptor.is_null() {
        log::warn!("USB descriptor failed: {error}");
        return;
    }
    // Host 库已校验并缓存了标准设备描述符；只按字节读取，避免 packed 字段未对齐引用。
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), 18) };
    let Some(identity) = DeviceIdentity::parse(bytes) else {
        log::warn!("invalid USB device descriptor");
        return;
    };
    log::info!(
        "USB device address={} VID={:04x} PID={:04x} USB={:04x} class={:02x} ({}) configurations={}",
        address,
        identity.vendor,
        identity.product,
        identity.usb_version,
        identity.class,
        class_name(identity.class),
        identity.configurations
    );

    let mut info = usb::usb_device_info_t::default();
    if unsafe { usb::usb_host_device_info(handle, &mut info) } == sys::ESP_OK {
        log::info!(
            "USB speed={} manufacturer={:?} product={:?}",
            info.speed,
            cached_string(info.str_desc_manufacturer),
            cached_string(info.str_desc_product)
        );
    }
    configuration(handle);
}

fn cached_string(descriptor: *const usb::usb_str_desc_t) -> Option<String> {
    if descriptor.is_null() {
        return None;
    }
    // 使用已缓存的字符串，既不发送额外控制写入，也不输出设备唯一序列号。
    let length = unsafe { descriptor.cast::<u8>().read() } as usize;
    if length < 2 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), length) };
    usb_descriptor::string(bytes)
}

fn configuration(handle: usb::usb_device_handle_t) {
    let mut descriptor = ptr::null();
    let error = unsafe { usb::usb_host_get_active_config_descriptor(handle, &mut descriptor) };
    if error != sys::ESP_OK || descriptor.is_null() {
        log::warn!("USB config failed: {error}");
        return;
    }
    let header = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), 9) };
    let length = u16::from_le_bytes([header[2], header[3]]) as usize;
    if length < 9 {
        log::warn!("invalid USB config length: {length}");
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), length) };
    let Some(entries) = Descriptors::configuration(bytes) else {
        log::warn!("invalid USB configuration");
        return;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            log::warn!("malformed USB descriptor entry");
            break;
        };
        match entry[1] {
            2 if entry.len() >= 9 => log::info!(
                "USB config interfaces={} power={}mA attributes={:02x}",
                entry[4],
                entry[8] as u16 * 2,
                entry[7]
            ),
            4 if entry.len() >= 9 => log::info!(
                "USB interface={} alternate={} endpoints={} class={:02x} ({}) subclass={:02x} protocol={:02x}",
                entry[2],
                entry[3],
                entry[4],
                entry[5],
                class_name(entry[5]),
                entry[6],
                entry[7]
            ),
            5 if entry.len() >= 7 => log::info!(
                "USB endpoint={:02x} transfer_type={} packet={} interval={}",
                entry[2],
                entry[3] & 3,
                u16::from_le_bytes([entry[4], entry[5]]),
                entry[6]
            ),
            _ => {}
        }
    }
}
