//! 单条连接的生命周期；管理器持有多条连接，不在这里决定输入目标。

use bt_switch_core::slots::PairingWindow;
use esp_idf_svc::sys;
use esp32_nimble::BLEServer;

const AUTH_TIMEOUT_MS: u64 = 30_000;
const RETRY_MS: u64 = 500;

pub(super) struct Connection {
    pub(super) handle: u16,
    pub(super) secured: bool,
    pub(super) closing: bool,
    /// 建连时捕获配对槽位及窗口，绝不在认证时借用后来的输入目标。
    pub(super) candidate: Option<(usize, PairingWindow)>,
    connected_at_ms: u64,
    security_requested: bool,
    next_attempt_ms: u64,
}

impl Connection {
    pub(super) fn new(handle: u16, candidate: Option<(usize, PairingWindow)>, now_ms: u64) -> Self {
        Self {
            handle,
            candidate,
            secured: false,
            closing: false,
            connected_at_ms: now_ms,
            security_requested: false,
            next_attempt_ms: now_ms.saturating_add(100),
        }
    }

    pub(super) fn security_pending(&self) -> bool {
        !self.secured && !self.closing && self.security_requested
    }

    pub(super) fn disconnect(&mut self, now_ms: u64) {
        self.closing = true;
        self.next_attempt_ms = now_ms;
    }

    pub(super) fn poll(&mut self, server: &mut BLEServer, can_request_security: bool, now_ms: u64) {
        if !self.secured && now_ms.saturating_sub(self.connected_at_ms) > AUTH_TIMEOUT_MS {
            if !self.closing {
                log::warn!("authentication conn={} timed out", self.handle);
            }
            self.closing = true;
        }

        if now_ms < self.next_attempt_ms {
            return;
        }
        if self.closing {
            self.next_attempt_ms = now_ms.saturating_add(RETRY_MS);
            if let Err(error) = server.disconnect(self.handle) {
                log::debug!("disconnect conn={} failed: {error:?}", self.handle);
            }
            // 不伪造断开：直到收到实际事件前仍占用容量，避免旧句柄被误复用。
        } else if !self.secured && !self.security_requested && can_request_security {
            let rc = unsafe { sys::ble_gap_security_initiate(self.handle) };
            self.next_attempt_ms = now_ms.saturating_add(RETRY_MS);
            if rc == 0 || rc == sys::BLE_HS_EALREADY as i32 {
                self.security_requested = true;
            } else {
                log::debug!("security conn={} retry: {rc}", self.handle);
            }
        }
    }
}
