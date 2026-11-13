//! USB 输入到 BLE 报告的转发管线，集中处理溢出、释放保护及发送确认。

use bt_switch_core::report::Translator;

use crate::ble::BleManager;
use crate::usb::{UsbEvent, UsbInput};

pub(super) struct InputPipeline {
    translator: Translator,
}

impl InputPipeline {
    pub(super) fn new() -> Self {
        Self {
            translator: Translator::new(),
        }
    }

    pub(super) fn disarm(&mut self) {
        self.translator.disarm();
    }

    pub(super) fn poll(&mut self, usb: &UsbInput, ble: &mut BleManager) {
        while let Ok(event) = usb.events.try_recv() {
            match event {
                UsbEvent::Input { source, state } => {
                    self.translator.update(usize::from(source), &state);
                }
                UsbEvent::Detached { source } => {
                    self.translator.detach(usize::from(source));
                }
            }
        }
        if usb.take_overflow() {
            log::warn!("USB input queue overflow: released all keys");
            self.translator.reset();
            ble.release_reports();
            // 溢出后丢弃队列，避免继续转发不完整的按键状态。
            while usb.events.try_recv().is_ok() {}
        }

        if ble.take_input_reset() {
            self.translator.disarm();
        }
        self.translator.set_ready(ble.is_ready());
        let reports = self.translator.flush();
        let sent = ble.send_reports(&reports);
        self.translator.confirm(&reports, sent);
    }
}
