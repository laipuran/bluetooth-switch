//! 输入目标选择、全释放重试及报告投递；传输通过闭包注入。

use super::{REPORTS, Router, report_bit, report_size};

impl Router {
    /// 只改变输入目标，不触碰连接；旧、新目标均需发送全释放。
    pub fn select(&mut self, slot: usize) -> bool {
        if slot >= self.slot_count || slot == self.selected {
            return false;
        }

        self.release_target();
        self.selected = slot;
        self.release_target();
        true
    }

    /// 切换后立即尝试释放；旧主机拥塞时仍可先完成新目标的释放。
    pub fn select_and_flush<F>(&mut self, slot: usize, notify: F) -> bool
    where
        F: FnMut(u16, u8, &[u8]) -> bool,
    {
        if !self.select(slot) {
            return false;
        }

        self.flush(notify);
        true
    }

    pub fn release_target(&mut self) {
        for peer in &mut self.peers {
            if peer.connected && peer.slot == Some(self.selected) {
                peer.releases |= peer.subscriptions;
            }
        }
    }

    /// 只有传输层确认成功排队才清除欠账，失败留待后续重试。
    pub fn flush<F>(&mut self, mut notify: F)
    where
        F: FnMut(u16, u8, &[u8]) -> bool,
    {
        let zero = [0u8; 8];

        for peer in &mut self.peers {
            if !peer.can_notify() {
                continue;
            }

            for report in 1..=REPORTS as u8 {
                let bit = report_bit(report);
                if peer.releases & peer.subscriptions & bit == 0 {
                    continue;
                }

                if notify(peer.connection, report, &zero[..report_size(report)]) {
                    peer.releases &= !bit;
                    peer.values[report as usize - 1] = [0; 8];
                }
            }
        }
    }

    pub fn ready(&self) -> bool {
        self.peers.iter().any(|peer| {
            peer.slot == Some(self.selected)
                && peer.can_notify()
                && peer.subscriptions != 0
                && peer.releases == 0
        })
    }

    /// 仅投递到当前目标；目标离线、不安全、挂起或退订时均不回退。
    pub fn send<F>(&mut self, report: u8, data: &[u8], mut notify: F) -> bool
    where
        F: FnMut(u16, u8, &[u8]) -> bool,
    {
        let size = report_size(report);
        if size == 0 || data.len() != size {
            return false;
        }

        let bit = report_bit(report);
        for peer in &mut self.peers {
            if peer.slot != Some(self.selected)
                || !peer.can_notify()
                || peer.releases != 0
                || peer.subscriptions & bit == 0
            {
                continue;
            }

            if !notify(peer.connection, report, data) {
                return false;
            }

            peer.values[report as usize - 1][..size].copy_from_slice(data);
            if report == 2 {
                // 相对位移只属于当前数据包，不得在读取或重试时重放。
                peer.values[1][1..].fill(0);
            }

            return true;
        }

        false
    }
}
