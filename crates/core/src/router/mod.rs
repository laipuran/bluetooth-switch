//! 多连接 HID 路由的纯逻辑核心，不依赖蓝牙栈。
//!
//! 在线集合与输入目标独立；切换只标记旧、新目标的全释放报告。
//! 发送失败的释放报告保留到后续轮询重试，不阻塞其他主机。

mod delivery;

#[cfg(test)]
mod tests;

use crate::MAX_SLOTS;

pub const MAX_PEERS: usize = MAX_SLOTS;
pub const REPORTS: usize = 3;
pub const SUB_KEYBOARD: u8 = 1 << 0;
pub const SUB_MOUSE: u8 = 1 << 1;
pub const SUB_CONSUMER: u8 = 1 << 2;

/// 每条连接独立的路由状态；尚未授权时 `slot` 为 `None`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub connected: bool,
    pub encrypted: bool,
    pub suspended: bool,
    pub congested: bool,
    pub connection: u16,
    pub slot: Option<usize>,
    pub subscriptions: u8,
    /// 每一位代表一个尚未成功排队的全释放报告。
    pub releases: u8,
    /// 各报告最后成功发送的值；鼠标相对位移不保留。
    pub values: [[u8; 8]; REPORTS],
}

impl Peer {
    fn can_notify(&self) -> bool {
        self.connected
            && self.encrypted
            && self.slot.is_some()
            && !self.suspended
            && !self.congested
    }

    pub fn is_subscribed(&self, report: u8) -> bool {
        report >= 1 && report as usize <= REPORTS && self.subscriptions & report_bit(report) != 0
    }
}

#[derive(Clone, Debug)]
pub struct Router {
    slot_count: usize,
    selected: usize,
    peers: [Peer; MAX_PEERS],
}

impl Router {
    pub fn new(slot_count: usize, selected: usize) -> Self {
        let slot_count = slot_count.clamp(1, MAX_PEERS);

        Self {
            slot_count,
            selected: if selected < slot_count { selected } else { 0 },
            peers: [Peer::default(); MAX_PEERS],
        }
    }

    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    pub fn peer(&self, connection: u16) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|peer| peer.connected && peer.connection == connection)
    }

    pub fn peer_mut(&mut self, connection: u16) -> Option<&mut Peer> {
        self.peers
            .iter_mut()
            .find(|peer| peer.connected && peer.connection == connection)
    }

    pub fn peer_for_slot(&self, slot: usize) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|peer| peer.connected && peer.slot == Some(slot))
    }

    pub fn connected_count(&self) -> usize {
        self.peers.iter().filter(|peer| peer.connected).count()
    }

    /// 登记连接；重复事件或容量不足时拒绝，不覆盖现有连接。
    pub fn connect(&mut self, connection: u16) -> bool {
        if self.peer(connection).is_some() {
            return false;
        }

        let Some(peer) = self.peers.iter_mut().find(|peer| !peer.connected) else {
            return false;
        };

        *peer = Peer {
            connected: true,
            connection,
            ..Default::default()
        };
        true
    }

    /// 按连接句柄移除状态；重复断开事件不会清理其他连接。
    pub fn disconnect(&mut self, connection: u16) -> bool {
        let Some(peer) = self.peer_mut(connection) else {
            return false;
        };

        *peer = Peer::default();
        true
    }

    /// 绑定槽位并标记已加密；调用方必须先验证协议栈的安全状态。
    /// 同一槽位不能被两条在线连接同时认领。
    pub fn authorize(&mut self, connection: u16, slot: usize) -> bool {
        if slot >= self.slot_count {
            return false;
        }

        let Some(index) = self
            .peers
            .iter()
            .position(|peer| peer.connected && peer.connection == connection)
        else {
            return false;
        };

        if self.peers[index].encrypted && self.peers[index].slot == Some(slot) {
            return true;
        }

        if self.peers.iter().any(|other| {
            other.connected && other.connection != connection && other.slot == Some(slot)
        }) {
            return false;
        }

        let peer = &mut self.peers[index];
        peer.slot = Some(slot);
        peer.encrypted = true;
        peer.releases |= peer.subscriptions;
        true
    }

    /// 更新单条连接的 CCCD 掩码；新订阅先发送全释放，退订则取消相应欠账。
    pub fn subscribe(&mut self, connection: u16, mask: u8) {
        let Some(peer) = self.peer_mut(connection) else {
            return;
        };

        let mask = mask & 0x07;
        peer.releases |= mask & !peer.subscriptions;
        peer.subscriptions = mask;
        peer.releases &= mask;
    }
}

pub fn report_size(report: u8) -> usize {
    match report {
        1 => 8,
        2 => 7,
        3 => 2,
        _ => 0,
    }
}

fn report_bit(report: u8) -> u8 {
    1u8 << (report.saturating_sub(1) & 7)
}
