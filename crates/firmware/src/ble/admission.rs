//! 主机按身份认领原槽位；新配对使用建连时捕获的窗口，不借用输入目标。

use super::connection::Connection;
use super::events::{BleEvent, format_address};
use super::{BleManager, SLOT_COUNT};

impl BleManager {
    pub(super) fn handle_event(&mut self, event: BleEvent, now_ms: u64) {
        match event {
            BleEvent::Connected { conn, address } => self.on_connected(conn, address, now_ms),
            BleEvent::Disconnected { conn } => self.on_disconnected(conn),
            BleEvent::Authenticated {
                conn,
                ok,
                address,
                address_type,
            } => self.on_authenticated(conn, ok, address, address_type, now_ms),
            BleEvent::SessionChanged { conn, release } => self.sync_session(conn, release),
        }
    }

    fn on_connected(&mut self, conn: u16, address: [u8; 6], now_ms: u64) {
        if self.connections.iter().any(|link| link.handle == conn) {
            return;
        }
        if self.connections.len() >= SLOT_COUNT || !self.router.connect(conn) {
            let _ = self.server.disconnect(conn);
            return;
        }

        let known = self.slots.find(address);
        let candidate = if known.is_none()
            && self.pairing_open(now_ms)
            && !self.connections.iter().any(|link| !link.secured)
        {
            self.pairing_slot.map(|slot| (slot, self.pairing))
        } else {
            None
        };
        let mut connection = Connection::new(conn, candidate, now_ms);
        if (known.is_none() && candidate.is_none())
            || known.is_some_and(|slot| self.forget_pending == Some(slot))
        {
            connection.disconnect(now_ms);
        }
        self.connections.push(connection);
        self.advertising.link_changed();
        log::info!(
            "host connected: {} conn={} online={}",
            format_address(address),
            conn,
            self.connections.len()
        );
    }

    fn on_disconnected(&mut self, conn: u16) {
        if self
            .router
            .peer(conn)
            .is_some_and(|peer| peer.slot == Some(self.slots.selected()))
        {
            self.input_reset = true;
        }
        self.connections.retain(|link| link.handle != conn);
        self.router.disconnect(conn);
        self.advertising.invalidate();
        log::info!(
            "host disconnected: conn={} online={}",
            conn,
            self.connections.len()
        );
    }

    fn on_authenticated(
        &mut self,
        conn: u16,
        ok: bool,
        address: [u8; 6],
        address_type: u8,
        now_ms: u64,
    ) {
        let Some(link) = self.connections.iter().find(|link| link.handle == conn) else {
            return;
        };
        if !ok || link.closing {
            if ok {
                self.cleanup_rejected_bond(address, address_type);
            }
            self.close_connection(conn, now_ms);
            return;
        }

        let slot = self.slots.find(address).or_else(|| {
            let (slot, window) = link.candidate?;
            (window.is_open(now_ms) && self.pairing_open(now_ms) && self.pairing_slot == Some(slot))
                .then_some(slot)
        });
        let Some(slot) = slot else {
            log::warn!("host {} has no authorized slot", format_address(address));
            self.cleanup_rejected_bond(address, address_type);
            self.close_connection(conn, now_ms);
            return;
        };

        if self.forget_pending == Some(slot) || !self.router.authorize(conn, slot) {
            self.close_connection(conn, now_ms);
            return;
        }

        if !self.slots.is_used(slot) {
            if let Err(error) = self.slots.bind(slot, address, address_type) {
                log::warn!("cannot bind slot {}: {error:?}", slot + 1);
                self.close_connection(conn, now_ms);
                return;
            }
            self.pairing.close();
            self.pairing_slot = None;
            self.save();
        }
        let link = self.connection_mut(conn).unwrap();
        link.secured = true;
        // 完成授权后不再属于临时配对候选，忘记其他空槽位不能误关此连接。
        link.candidate = None;
        self.transport
            .sessions
            .update(conn, |state| state.authorized = true);
        self.sync_session(conn, true);
        self.advertising.invalidate();
        log::info!(
            "slot {} encrypted: conn={} online={}",
            slot + 1,
            conn,
            self.connections.len()
        );
    }

    fn sync_session(&mut self, conn: u16, release: bool) {
        let Some(session) = self.transport.sessions.get(conn) else {
            return;
        };
        self.router.subscribe(conn, session.subscriptions());
        if let Some(peer) = self.router.peer_mut(conn) {
            let changed = peer.suspended != session.suspended || release;
            peer.suspended = session.suspended;
            peer.encrypted &= session.encrypted;
            if changed {
                peer.releases |= peer.subscriptions;
            }
            if peer.slot == Some(self.slots.selected()) {
                self.input_reset = true;
            }
        }
    }

    pub(super) fn close_connection(&mut self, conn: u16, now_ms: u64) {
        if let Some(link) = self.connection_mut(conn) {
            link.disconnect(now_ms);
        }
        if let Some(peer) = self.router.peer_mut(conn) {
            peer.encrypted = false;
        }
        self.transport
            .sessions
            .update(conn, |state| state.authorized = false);
    }

    fn cleanup_rejected_bond(&mut self, address: [u8; 6], address_type: u8) {
        // 只清理未认领槽位的新配对，不能误删其他合法主机的密钥。
        if self.slots.find(address).is_none()
            && let Err(error) = self.remove_peer_bond(address, address_type)
        {
            log::warn!("cannot remove rejected bond: {error:?}");
        }
    }
}
