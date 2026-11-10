//! NVS-backed persistence for the host slot table.

use anyhow::Result;
use bt_switch_core::slots::{STORAGE_LEN, SlotTable};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

const NAMESPACE: &str = "bt_switch";
const KEY: &str = "slots";

pub struct Storage {
    nvs: EspNvs<NvsDefault>,
}

impl Storage {
    pub fn new(partition: EspDefaultNvsPartition) -> Result<Self> {
        Ok(Self {
            nvs: EspNvs::new(partition, NAMESPACE, true)?,
        })
    }

    pub fn load(&self, slot_count: usize) -> SlotTable {
        let mut buffer = [0u8; STORAGE_LEN];
        match self.nvs.get_blob(KEY, &mut buffer) {
            Ok(Some(bytes)) => SlotTable::decode(bytes, slot_count).unwrap_or_else(|| {
                log::warn!("stored slot table is invalid; starting with empty slots");
                SlotTable::new(slot_count)
            }),
            Ok(None) => SlotTable::new(slot_count),
            Err(err) => {
                log::error!("cannot read slot table: {err}");
                SlotTable::new(slot_count)
            }
        }
    }

    pub fn save(&self, slots: &SlotTable) {
        if let Err(err) = self.nvs.set_blob(KEY, &slots.encode()) {
            log::error!("cannot save slot table: {err}");
        }
    }
}
