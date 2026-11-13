//! USB 输入到 BLE 报告的转发管线，集中处理溢出、释放保护及发送确认。

use std::time::Duration;

use anyhow::{Result, anyhow};
use bt_switch_core::report::Translator;

use super::inbox;

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

    pub(super) fn poll(&mut self, usb: &UsbInput, ble: &mut BleManager) -> Result<()> {
        // 无输入时阻塞至多 1 ms；有输入立即返回，不再固定睡眠 4 ms。
        // 每批有上限，避免持续输入让 BLE 管理、串口和按键得不到运行机会。
        if let Err(error) = inbox::receive_batch(&usb.events, Duration::from_millis(1), |event| {
            self.apply_event(event);
        }) {
            self.translator.reset();
            ble.release_reports();
            return Err(anyhow!("USB input channel disconnected: {error}"));
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
        Ok(())
    }

    fn apply_event(&mut self, event: UsbEvent) {
        match event {
            UsbEvent::Input { source, state } => {
                self.translator.update(usize::from(source), &state);
            }
            UsbEvent::Detached { source } => {
                self.translator.detach(usize::from(source));
            }
        }
    }
}
