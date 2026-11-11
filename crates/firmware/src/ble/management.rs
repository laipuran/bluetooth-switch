//! 输入目标、配对窗口和忘记槽位是三种独立操作；只有忘记会断开对应连接。

use super::events::format_address;
use super::{BleManager, PAIRING_SECONDS};

impl BleManager {
    pub fn select(&mut self, slot: usize, now_ms: u64) -> bool {
        if !self.slots.select(slot) {
            return false;
        }
        self.slots.commit();
        let transport = &self.transport;
        self.router.select_and_flush(slot, |conn, report, data| {
            transport.notify(conn, report, data)
        });
        self.input_reset = true;
        if !self.slots.is_used(slot) && self.forget_pending != Some(slot) {
            self.open_pairing(slot, now_ms);
        }
        self.save();
        log::info!(
            "selected input slot {} ({} links retained)",
            slot + 1,
            self.connections.len()
        );
        true
    }

    pub fn next_slot(&mut self, now_ms: u64) -> bool {
        self.select((self.slots.selected() + 1) % self.slots.count(), now_ms)
    }

    pub fn prev_slot(&mut self, now_ms: u64) -> bool {
        self.select(
            (self.slots.selected() + self.slots.count() - 1) % self.slots.count(),
            now_ms,
        )
    }

    pub fn pair(&mut self, now_ms: u64) {
        let empty = (0..self.slots.count())
            .map(|offset| (self.slots.selected() + offset) % self.slots.count())
            .find(|slot| !self.slots.is_used(*slot) && self.forget_pending != Some(*slot));
        if let Some(slot) = empty {
            self.open_pairing(slot, now_ms);
        } else {
            log::warn!("no empty slot; forget one before pairing");
        }
    }

    pub(super) fn open_pairing(&mut self, slot: usize, now_ms: u64) {
        self.pairing_slot = Some(slot);
        self.pairing.open(now_ms, PAIRING_SECONDS);
        self.advertising.invalidate();
        log::info!(
            "pairing open on slot {} for {} s; input target unchanged",
            slot + 1,
            PAIRING_SECONDS
        );
    }

    pub fn forget(&mut self, now_ms: u64) {
        if self.forget_pending.is_some() {
            return;
        }
        let slot = self.slots.selected();
        self.release_reports();
        self.forget_pending = Some(slot);
        self.forget_retry_ms = 0;
        self.input_reset = true;
        let handles: Vec<_> = self
            .connections
            .iter()
            .filter(|link| {
                self.router
                    .peer(link.handle)
                    .is_some_and(|peer| peer.slot == Some(slot))
                    || link.candidate.is_some_and(|candidate| candidate.0 == slot)
            })
            .map(|link| link.handle)
            .collect();
        for handle in handles {
            self.close_connection(handle, now_ms);
        }
        self.advertising.invalidate();
    }

    pub(super) fn finish_forget(&mut self, now_ms: u64) {
        let Some(slot) = self.forget_pending else {
            return;
        };
        if now_ms < self.forget_retry_ms {
            return;
        }
        if self.connections.iter().any(|link| {
            self.router
                .peer(link.handle)
                .is_some_and(|peer| peer.slot == Some(slot))
                || link.candidate.is_some_and(|candidate| candidate.0 == slot)
        }) {
            return;
        }

        if let Some(saved) = self.slots.slot(slot).filter(|saved| saved.used).copied()
            && let Err(error) = self.remove_peer_bond(saved.address, saved.address_type)
        {
            self.forget_retry_ms = now_ms.saturating_add(1000);
            log::warn!("cannot delete bond for slot {}: {error:?}", slot + 1);
            return;
        }
        self.slots.forget(slot);
        self.forget_pending = None;
        self.save();
        self.open_pairing(slot, now_ms);
        log::warn!(
            "forgot slot {}; remove BT Switch on that host too",
            slot + 1
        );
    }

    pub fn status(&self, now_ms: u64) {
        log::info!(
            "slot {}/{} online={} ready={} advertising={} pairing_slot={:?} pairing={}s",
            self.slots.selected() + 1,
            self.slots.count(),
            self.connections.len(),
            self.is_ready(),
            self.advertising.is_active(),
            self.pairing_slot.map(|slot| slot + 1),
            self.pairing.remaining_ms(now_ms) / 1000
        );
        for (index, slot) in self.slots.slots().iter().enumerate() {
            log::info!(
                "  {}: {} {}",
                index + 1,
                if slot.used { "paired" } else { "empty" },
                format_address(slot.address)
            );
        }
        for peer in self.router.peers().iter().filter(|peer| peer.connected) {
            log::info!(
                "  conn={} slot={:?} encrypted={} subs={:#04x} suspended={} releases={:#04x}",
                peer.connection,
                peer.slot.map(|slot| slot + 1),
                peer.encrypted,
                peer.subscriptions,
                peer.suspended,
                peer.releases
            );
        }
    }

    pub(super) fn save(&self) {
        self.storage.save(&self.slots);
    }
}
