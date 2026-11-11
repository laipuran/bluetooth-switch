//! 定向 HID 通知：不使用会向所有订阅者广播的 `.notify()`。

use std::sync::mpsc::Sender;

use bt_switch_core::session::boot_mouse;

use super::attributes::Sessions;
use super::events::BleEvent;
use super::profile::HidProfile;

mod callbacks;

pub(super) struct HidTransport {
    profile: HidProfile,
    pub(super) sessions: Sessions,
}

impl HidTransport {
    pub(super) fn new(profile: HidProfile, sessions: Sessions, tx: Sender<BleEvent>) -> Self {
        callbacks::register(&profile, &sessions, &tx);
        Self { profile, sessions }
    }

    /// 成功仅表示 NimBLE 接受排队；失败会保留路由器的释放欠账供下次重试。
    pub(super) fn notify(&self, connection: u16, report: u8, data: &[u8]) -> bool {
        let Some(session) = self.sessions.get(connection) else {
            return false;
        };
        if !session.can_send(report) {
            return false;
        }

        let boot = session.protocol == 0;
        let characteristic = if boot {
            match report {
                1 => &self.profile.boot_keyboard,
                2 => &self.profile.boot_mouse,
                _ => return false,
            }
        } else {
            &self.profile.inputs[report as usize - 1]
        };
        let mouse;
        let payload = if boot && report == 2 {
            let Some(value) = boot_mouse(data) else {
                return false;
            };
            mouse = value;
            &mouse[..]
        } else {
            data
        };

        // 不持有 Sessions 锁调用 NimBLE，避免与栈的特征锁形成反向锁顺序。
        if let Err(error) = characteristic.lock().notify_with(payload, connection) {
            log::debug!("notify conn={connection} report={report} failed: {error:?}");
            return false;
        }

        self.sessions
            .update(connection, |state| state.remember(report, data));
        true
    }
}
