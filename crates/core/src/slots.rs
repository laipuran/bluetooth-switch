//! Persistent host-slot bookkeeping and the pairing-window timer.

use crate::MAX_SLOTS;

const STORAGE_VERSION: u32 = 1;
/// Fixed on-flash size of the slot table blob.
pub const STORAGE_LEN: usize = 4 + 1 + MAX_SLOTS * 8;

/// One bonded Bluetooth host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub used: bool,
    /// Address in NimBLE's little-endian byte order.
    pub address: [u8; 6],
    /// `BLEAddressType` value (0 public, 1 random, 2 public ID, 3 random ID).
    pub address_type: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindError {
    InvalidSlot,
    Occupied,
    Duplicate(usize),
}

/// The configured slots plus the selected/desired slot indices.
#[derive(Clone, Debug)]
pub struct SlotTable {
    slots: [Slot; MAX_SLOTS],
    count: usize,
    selected: usize,
    desired: usize,
}

impl SlotTable {
    pub fn new(count: usize) -> Self {
        assert!((1..=MAX_SLOTS).contains(&count), "slot count out of range");
        Self {
            slots: [Slot::default(); MAX_SLOTS],
            count,
            selected: 0,
            desired: 0,
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn desired(&self) -> usize {
        self.desired
    }

    pub fn slot(&self, index: usize) -> Option<&Slot> {
        (index < self.count).then(|| &self.slots[index])
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots[..self.count]
    }

    pub fn is_used(&self, index: usize) -> bool {
        index < self.count && self.slots[index].used
    }

    /// Request a slot. The switch is committed on the next disconnect.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.count || index == self.desired {
            return false;
        }
        self.desired = index;
        true
    }

    /// Commit a pending selection. Returns true when the selection changed.
    pub fn commit(&mut self) -> bool {
        if self.selected == self.desired {
            return false;
        }
        self.selected = self.desired;
        true
    }

    /// Store a newly paired host in an empty slot.
    pub fn bind(
        &mut self,
        index: usize,
        address: [u8; 6],
        address_type: u8,
    ) -> Result<(), BindError> {
        if index >= self.count {
            return Err(BindError::InvalidSlot);
        }
        if self.slots[index].used {
            return Err(BindError::Occupied);
        }
        if let Some(other) = self.find(address) {
            return Err(BindError::Duplicate(other));
        }
        self.slots[index] = Slot {
            used: true,
            address,
            address_type,
        };
        Ok(())
    }

    /// Remove a host from a slot.
    pub fn forget(&mut self, index: usize) {
        if index < self.count {
            self.slots[index] = Slot::default();
        }
    }

    /// Find a host address in any slot.
    pub fn find(&self, address: [u8; 6]) -> Option<usize> {
        self.slots[..self.count]
            .iter()
            .position(|slot| slot.used && slot.address == address)
    }

    /// Whether a slot contains the given host address.
    pub fn matches(&self, index: usize, address: [u8; 6]) -> bool {
        index < self.count && self.slots[index].used && self.slots[index].address == address
    }

    pub fn encode(&self) -> [u8; STORAGE_LEN] {
        let mut bytes = [0u8; STORAGE_LEN];
        bytes[..4].copy_from_slice(&STORAGE_VERSION.to_le_bytes());
        bytes[4] = self.selected as u8;
        for (index, slot) in self.slots.iter().enumerate() {
            let offset = 5 + index * 8;
            bytes[offset] = u8::from(slot.used);
            bytes[offset + 1] = slot.address_type;
            bytes[offset + 2..offset + 8].copy_from_slice(&slot.address);
        }
        bytes
    }

    /// Decode a stored blob. Any inconsistency falls back to a fresh table.
    pub fn decode(bytes: &[u8], count: usize) -> Option<Self> {
        if !(1..=MAX_SLOTS).contains(&count) || bytes.len() != STORAGE_LEN {
            return None;
        }
        let version = u32::from_le_bytes(bytes[..4].try_into().ok()?);
        if version != STORAGE_VERSION {
            return None;
        }
        let selected = usize::from(bytes[4]);
        if selected >= count {
            return None;
        }

        let mut table = Self::new(count);
        table.selected = selected;
        table.desired = selected;
        for index in 0..count {
            let offset = 5 + index * 8;
            let used = bytes[offset] != 0;
            let address_type = bytes[offset + 1];
            if address_type > 3 {
                return None;
            }
            let mut address = [0u8; 6];
            address.copy_from_slice(&bytes[offset + 2..offset + 8]);
            if used {
                table.slots[index] = Slot {
                    used,
                    address,
                    address_type,
                };
            }
        }
        Some(table)
    }
}

/// A deadline measured in milliseconds since boot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PairingWindow {
    deadline_ms: Option<u64>,
}

impl PairingWindow {
    pub fn open(&mut self, now_ms: u64, seconds: u64) {
        self.deadline_ms = Some(now_ms.saturating_add(seconds.saturating_mul(1000)));
    }

    pub fn extend(&mut self, now_ms: u64, seconds: u64) {
        let deadline = now_ms.saturating_add(seconds.saturating_mul(1000));
        if self.deadline_ms.is_none_or(|current| deadline > current) {
            self.deadline_ms = Some(deadline);
        }
    }

    pub fn close(&mut self) {
        self.deadline_ms = None;
    }

    pub fn is_open(&self, now_ms: u64) -> bool {
        self.deadline_ms.is_some_and(|deadline| now_ms < deadline)
    }

    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.deadline_ms
            .map_or(0, |deadline| deadline.saturating_sub(now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(last: u8) -> [u8; 6] {
        [1, 2, 3, 4, 5, last]
    }

    #[test]
    fn select_and_commit() {
        let mut table = SlotTable::new(3);
        assert_eq!(table.selected(), 0);
        assert!(!table.select(0));
        assert!(table.select(2));
        assert_eq!(table.desired(), 2);
        assert_eq!(table.selected(), 0);
        assert!(table.commit());
        assert_eq!(table.selected(), 2);
        assert!(!table.commit());
    }

    #[test]
    fn bind_rejects_occupied_and_duplicate_addresses() {
        let mut table = SlotTable::new(3);
        assert_eq!(table.bind(3, addr(1), 0), Err(BindError::InvalidSlot));
        assert_eq!(table.bind(0, addr(1), 0), Ok(()));
        assert!(table.is_used(0));
        assert_eq!(table.bind(0, addr(2), 0), Err(BindError::Occupied));
        assert_eq!(table.bind(1, addr(1), 1), Err(BindError::Duplicate(0)));
        assert_eq!(table.find(addr(1)), Some(0));
        assert!(table.matches(0, addr(1)));
        assert!(!table.matches(1, addr(1)));

        table.forget(1);
        assert!(!table.is_used(1));
        table.forget(0);
        assert_eq!(table.find(addr(1)), None);
    }

    #[test]
    fn storage_round_trip_and_validation() {
        let mut table = SlotTable::new(3);
        table.bind(0, addr(1), 0).unwrap();
        table.bind(2, addr(9), 3).unwrap();
        table.select(2);
        table.commit();

        let bytes = table.encode();
        assert_eq!(bytes.len(), STORAGE_LEN);
        let restored = SlotTable::decode(&bytes, 3).unwrap();
        assert_eq!(restored.selected(), 2);
        assert_eq!(restored.desired(), 2);
        assert_eq!(restored.slot(0).unwrap().address, addr(1));
        assert_eq!(restored.slot(2).unwrap().address_type, 3);

        assert!(SlotTable::decode(&bytes, 2).is_none());
        assert!(SlotTable::decode(&bytes[..STORAGE_LEN - 1], 3).is_none());

        let mut bad_version = bytes;
        bad_version[0] = 2;
        assert!(SlotTable::decode(&bad_version, 3).is_none());

        let mut bad_selected = bytes;
        bad_selected[4] = 3;
        assert!(SlotTable::decode(&bad_selected, 3).is_none());

        let mut bad_type = bytes;
        bad_type[5 + 1] = 9;
        assert!(SlotTable::decode(&bad_type, 3).is_none());
    }

    #[test]
    fn pairing_window_expires() {
        let mut window = PairingWindow::default();
        assert!(!window.is_open(0));
        window.open(1000, 120);
        assert!(window.is_open(1000));
        assert!(window.is_open(120_999));
        assert!(!window.is_open(121_000));
        assert_eq!(window.remaining_ms(120_000), 1000);
        window.close();
        assert!(!window.is_open(0));
    }
}
