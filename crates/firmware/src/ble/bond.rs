//! 删除单个主机密钥的栈前置条件：暂停广播，但不拆除其他在线连接。

use esp_idf_svc::sys;
use esp32_nimble::BLEError;

use super::BleManager;
use super::events::to_ble_address;

impl BleManager {
    pub(super) fn remove_peer_bond(
        &mut self,
        address: [u8; 6],
        address_type: u8,
    ) -> Result<(), BLEError> {
        // ESP-IDF 6.1 ble_gap_unpair 在广播活跃时直接返回 EBUSY。
        // 广播由下一次 poll 按当前槽位和配对窗口恢复，不调用全局断开。
        self.advertising.pause(self.device)?;
        match self
            .device
            .delete_bond(&to_ble_address(address, address_type))
        {
            Err(error) if error.code() == sys::BLE_HS_ENOENT => Ok(()),
            result => result,
        }
    }
}
