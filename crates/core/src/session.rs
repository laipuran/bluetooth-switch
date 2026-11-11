//! 每条 HID 连接独立的动态属性与协议状态，不共享主机的可变值。

use crate::router::{REPORTS, report_size};

#[derive(Clone, Debug)]
pub struct HidSession {
    pub encrypted: bool,
    pub authorized: bool,
    pub protocol: u8,
    pub suspended: bool,
    pub leds: u8,
    report_subscriptions: u8,
    boot_subscriptions: u8,
    values: [[u8; 8]; REPORTS],
}

impl Default for HidSession {
    fn default() -> Self {
        Self {
            encrypted: false,
            authorized: false,
            protocol: 1,
            suspended: false,
            leds: 0,
            report_subscriptions: 0,
            boot_subscriptions: 0,
            values: [[0; 8]; REPORTS],
        }
    }
}

impl HidSession {
    pub fn subscribe(&mut self, report: u8, boot: bool, enabled: bool) {
        if report_size(report) == 0 || (boot && report == 3) {
            return;
        }

        let mask = if boot {
            &mut self.boot_subscriptions
        } else {
            &mut self.report_subscriptions
        };
        let bit = 1 << (report - 1);
        if enabled {
            *mask |= bit;
        } else {
            *mask &= !bit;
        }
    }

    pub fn subscriptions(&self) -> u8 {
        if self.protocol == 0 {
            self.boot_subscriptions
        } else {
            self.report_subscriptions
        }
    }

    pub fn can_send(&self, report: u8) -> bool {
        self.authorized
            && self.encrypted
            && !self.suspended
            && report_size(report) != 0
            && self.subscriptions() & (1 << (report - 1)) != 0
    }

    /// 保存已排队的状态，但绝不保存相对位移供后续读取重放。
    pub fn remember(&mut self, report: u8, data: &[u8]) {
        if report_size(report) == 0 || data.len() != report_size(report) {
            return;
        }
        self.values[report as usize - 1][..data.len()].copy_from_slice(data);
        if report == 2 {
            self.values[1][1..].fill(0);
        }
    }

    pub fn value(&self, report: u8) -> &[u8] {
        let size = report_size(report);
        if size == 0 {
            &[]
        } else {
            &self.values[report as usize - 1][..size]
        }
    }
}

/// Boot 鼠标只有按钮和 8 位 XY；不重解释成无符号数，也不携带滚轮。
pub fn boot_mouse(data: &[u8]) -> Option<[u8; 3]> {
    if data.len() != 7 {
        return None;
    }
    let x = i16::from_le_bytes([data[1], data[2]]).clamp(-127, 127) as i8;
    let y = i16::from_le_bytes([data[3], data[4]]).clamp(-127, 127) as i8;
    // Boot 鼠标只有三个按钮，高五位是保留位。
    Some([data[0] & 0x07, x as u8, y as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_subscription_suspend_and_leds_are_peer_local() {
        let mut first = HidSession {
            authorized: true,
            encrypted: true,
            ..Default::default()
        };
        let mut second = first.clone();
        first.subscribe(1, false, true);
        second.subscribe(2, true, true);
        second.protocol = 0;
        second.leds = 3;
        second.suspended = true;
        assert!(first.can_send(1));
        assert!(!first.can_send(2));
        assert_eq!(first.leds, 0);
        assert_eq!(first.protocol, 1);
        assert!(!second.can_send(2));
        second.suspended = false;
        assert!(second.can_send(2));
        second.protocol = 1;
        assert!(!second.can_send(2));
        first.encrypted = false;
        assert!(!first.can_send(1));
    }

    #[test]
    fn relative_motion_is_not_replayed_and_boot_conversion_is_signed() {
        let mut session = HidSession::default();
        let mouse = [5, 0xff, 0x7f, 0x00, 0x80, 3, 4];
        assert_eq!(boot_mouse(&mouse), Some([5, 127, (-127i8) as u8]));
        assert_eq!(boot_mouse(&[0xff, 0, 0, 0, 0, 0, 0]), Some([7, 0, 0]));
        session.remember(2, &mouse);
        assert_eq!(session.value(2), &[5, 0, 0, 0, 0, 0, 0]);
        assert_eq!(session.value(0), &[]);
    }
}
