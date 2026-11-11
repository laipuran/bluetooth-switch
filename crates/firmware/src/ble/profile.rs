//! HID GATT 布局：静态属性与需加密的动态属性在这里集中定义。
//!
//! 不依赖 helper 的全局 protocol/control 值，避免主机间共享可变状态。

use std::sync::Arc;

use bt_switch_core::hid::REPORT_MAP;
use esp32_nimble::utilities::{BleUuid, mutex::Mutex};
use esp32_nimble::{
    BLECharacteristic, BLEServer, BLEService, DescriptorProperties, NimbleProperties as P,
};

pub(super) type Characteristic = Arc<Mutex<BLECharacteristic>>;

pub(super) struct HidProfile {
    pub(super) inputs: [Characteristic; 3],
    pub(super) boot_keyboard: Characteristic,
    pub(super) boot_mouse: Characteristic,
    pub(super) outputs: [Characteristic; 2],
    pub(super) protocol: Characteristic,
    pub(super) control: Characteristic,
}

impl HidProfile {
    pub(super) fn new(server: &mut BLEServer) -> Self {
        // 保留既有 Rust 服务及报告的创建顺序，Boot 属性追加到 HID 服务尾部。
        let info = server.create_service(BleUuid::from_uuid16(0x180a));
        let hid = server.create_service(BleUuid::from_uuid16(0x1812));
        let battery = server.create_service(BleUuid::from_uuid16(0x180f));

        characteristic(&info, 0x2a50, P::READ)
            .lock()
            .set_value(&[2, 0xff, 0xff, 0, 1, 1, 0]);
        characteristic(&info, 0x2a29, P::READ)
            .lock()
            .set_value(b"DIY");
        characteristic(&hid, 0x2a4a, P::READ)
            .lock()
            .set_value(&[0x11, 1, 0, 2]);
        characteristic(&hid, 0x2a4b, P::READ)
            .lock()
            .set_value(REPORT_MAP);

        let control = characteristic(&hid, 0x2a4c, P::WRITE_NO_RSP | P::WRITE_ENC);
        let protocol = characteristic(
            &hid,
            0x2a4e,
            P::READ | P::READ_ENC | P::WRITE_NO_RSP | P::WRITE_ENC,
        );
        let inputs = core::array::from_fn(|index| {
            let input = characteristic(
                &hid,
                0x2a4d,
                P::READ | P::READ_ENC | P::NOTIFY | P::NOTIFY_INDICATE_ENC,
            );
            input
                .lock()
                .create_descriptor(
                    BleUuid::from_uuid16(0x2908),
                    DescriptorProperties::READ | DescriptorProperties::READ_ENC,
                )
                .lock()
                .set_value(&[index as u8 + 1, 1]);
            input
        });
        let output = characteristic(
            &hid,
            0x2a4d,
            P::READ | P::READ_ENC | P::WRITE | P::WRITE_NO_RSP | P::WRITE_ENC,
        );
        output
            .lock()
            .create_descriptor(
                BleUuid::from_uuid16(0x2908),
                DescriptorProperties::READ | DescriptorProperties::READ_ENC,
            )
            .lock()
            .set_value(&[1, 2]);

        let boot_keyboard = characteristic(
            &hid,
            0x2a22,
            P::READ | P::READ_ENC | P::NOTIFY | P::NOTIFY_INDICATE_ENC,
        );
        let boot_mouse = characteristic(
            &hid,
            0x2a33,
            P::READ | P::READ_ENC | P::NOTIFY | P::NOTIFY_INDICATE_ENC,
        );
        let boot_output = characteristic(
            &hid,
            0x2a32,
            P::READ | P::READ_ENC | P::WRITE | P::WRITE_NO_RSP | P::WRITE_ENC,
        );

        let level = characteristic(&battery, 0x2a19, P::READ | P::NOTIFY);
        level.lock().set_value(&[100]);
        level
            .lock()
            .create_descriptor(BleUuid::from_uuid16(0x2904), DescriptorProperties::READ)
            .lock()
            .set_value(&[4, 0, 0xad, 0x27, 1, 0, 0]);

        Self {
            inputs,
            boot_keyboard,
            boot_mouse,
            outputs: [output, boot_output],
            protocol,
            control,
        }
    }
}

fn characteristic(service: &Arc<Mutex<BLEService>>, uuid: u16, properties: P) -> Characteristic {
    service
        .lock()
        .create_characteristic(BleUuid::from_uuid16(uuid), properties)
}
