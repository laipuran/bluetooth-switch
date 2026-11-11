//! 广播数据、白名单及控制器启停重试；不负责断开连接或保存槽位。

use anyhow::{Result, anyhow};
use bt_switch_core::slots::SlotTable;
use esp32_nimble::enums::AdvFilterPolicy;
use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEAdvertisementData, BLEDevice};

use super::events::to_ble_address;

pub(super) struct Advertising {
    active: bool,
    dirty: bool,
    retry_at_ms: u64,
    pairing: bool,
}

impl Advertising {
    pub(super) fn new(device: &mut BLEDevice) -> Result<Self> {
        let advertising = device.get_advertising();
        advertising.lock().scan_response(false);

        let mut advertisement = BLEAdvertisementData::new();
        advertisement
            .name("BT Switch")
            .appearance(0x03c0)
            .add_service_uuid(BleUuid::from_uuid16(0x1812));

        advertising
            .lock()
            .set_data(&mut advertisement)
            .map_err(|err| anyhow!("cannot set advertising data: {err:?}"))?;

        device
            .set_white_list(&[])
            .map_err(|err| anyhow!("cannot clear whitelist: {err:?}"))?;

        Ok(Self {
            active: false,
            dirty: true,
            retry_at_ms: 0,
            pairing: false,
        })
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
        self.retry_at_ms = 0;
    }

    /// 建连会终止广播；断连回调也要求重新检查控制器状态。
    pub(super) fn link_changed(&mut self) {
        self.invalidate();
    }

    pub(super) fn poll(
        &mut self,
        device: &mut BLEDevice,
        allowed: bool,
        slots: &SlotTable,
        pairing: bool,
        now_ms: u64,
    ) {
        // 建连会结束广播，断连时却可能仍在广播；始终以栈的实际状态为准。
        self.active = device.get_advertising().lock().is_advertising();
        if self.pairing != pairing {
            self.pairing = pairing;
            self.invalidate();
        }
        if self.active && (!allowed || self.dirty) {
            self.stop(device);
        }

        if !self.active && allowed && now_ms >= self.retry_at_ms {
            self.start(device, slots, now_ms);
        }
    }

    fn start(&mut self, device: &mut BLEDevice, slots: &SlotTable, now_ms: u64) {
        let filtered = configure_whitelist(device, slots, self.pairing);
        let advertising = device.get_advertising();
        let mut advertising = advertising.lock();
        advertising.filter_policy(if filtered {
            AdvFilterPolicy::Connect
        } else {
            AdvFilterPolicy::None
        });

        match advertising.start() {
            Ok(()) => {
                self.active = true;
                self.dirty = false;
                self.retry_at_ms = 0;
                log::info!(
                    "advertising ({})",
                    if !self.pairing {
                        "all paired hosts"
                    } else {
                        "PAIRING OPEN"
                    }
                );
            }
            Err(err) if advertising.is_advertising() => {
                // EALREADY 等竞争结果以控制器实际状态为准。
                log::debug!("advertising already active: {err:?}");
                self.active = true;
                self.dirty = false;
                self.retry_at_ms = 0;
            }
            Err(err) => {
                self.active = false;
                self.retry_at_ms = now_ms.saturating_add(1000);
                log::warn!("cannot start advertising: {err:?}");
            }
        }
    }

    fn stop(&mut self, device: &mut BLEDevice) {
        let advertising = device.get_advertising();
        let advertising = advertising.lock();

        match advertising.stop() {
            Ok(()) => self.active = false,
            Err(err) => {
                log::warn!("cannot stop advertising: {err:?}");
                self.active = advertising.is_advertising();
            }
        }
    }
}

fn configure_whitelist(device: &mut BLEDevice, slots: &SlotTable, pairing: bool) -> bool {
    if !pairing {
        let addresses: Vec<_> = slots
            .slots()
            .iter()
            .filter(|slot| slot.used)
            .map(|slot| to_ble_address(slot.address, slot.address_type))
            .collect();
        match device.set_white_list(&addresses) {
            Ok(()) => return true,
            Err(err) => {
                log::warn!("cannot set whitelist: {err:?}; using software validation");
            }
        }
    }

    let _ = device.set_white_list(&[]);
    false
}
