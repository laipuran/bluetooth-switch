//! Bluetooth HID peripheral, host slots and advertising state machine.
//!
//! The firmware is a BLE HID device with three report IDs (keyboard, mouse,
//! consumer). One bonded host is active at a time; selecting another slot
//! disconnects the current host, advertises with a one-entry whitelist and
//! waits for the new host. Empty slots can advertise during a limited pairing
//! window, mirroring the C prototype.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Result, anyhow};
use bt_switch_core::hid::{CONSUMER_REPORT_ID, KEYBOARD_REPORT_ID, MOUSE_REPORT_ID, REPORT_MAP};
use bt_switch_core::report::{Reports, Sent, ZERO_CONSUMER, ZERO_KEYBOARD, ZERO_MOUSE};
use bt_switch_core::slots::{BindError, PairingWindow, SlotTable};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys;
use esp32_nimble::enums::{AdvFilterPolicy, AuthReq, SecurityIOCap};
use esp32_nimble::utilities::BleUuid;
use esp32_nimble::utilities::mutex::Mutex;
use esp32_nimble::{
    BLEAddress, BLEAddressType, BLEAdvertisementData, BLECharacteristic, BLEConnDesc, BLEDevice,
    BLEHIDDevice, BLEServer,
};

use crate::storage::Storage;

/// Number of hosts that can be selected from the UI.
pub const SLOT_COUNT: usize = 3;
const PAIRING_SECONDS: u64 = 120;
const AUTH_TIMEOUT_MS: u64 = 30_000;
const READY_DELAY_MS: u64 = 800;
const SECURITY_RETRY_MS: u64 = 500;
const SECURITY_ATTEMPTS: u8 = 3;

#[derive(Debug)]
enum BleEvent {
    Connected {
        conn: u16,
        address: [u8; 6],
        address_type: u8,
    },
    Disconnected {
        conn: u16,
    },
    Authenticated {
        conn: u16,
        ok: bool,
        address: [u8; 6],
        address_type: u8,
    },
}

pub struct BleManager {
    device: &'static mut BLEDevice,
    server: &'static mut BLEServer,
    keyboard: Arc<Mutex<BLECharacteristic>>,
    mouse: Arc<Mutex<BLECharacteristic>>,
    consumer: Arc<Mutex<BLECharacteristic>>,
    _output: Arc<Mutex<BLECharacteristic>>,
    events: Receiver<BleEvent>,
    storage: Storage,
    slots: SlotTable,
    pairing: PairingWindow,
    connected: bool,
    conn: u16,
    secured: bool,
    ready: bool,
    ready_at_ms: u64,
    connect_at_ms: u64,
    security_requested: bool,
    security_attempts: u8,
    next_security_ms: u64,
    advertising: bool,
    advertising_dirty: bool,
    next_advertise_ms: u64,
    disconnect_requested: bool,
    next_disconnect_ms: u64,
    disconnect_deadline_ms: u64,
    forget_pending: bool,
}

impl BleManager {
    pub fn new(partition: EspDefaultNvsPartition, now_ms: u64) -> Result<Self> {
        let storage = Storage::new(partition)?;
        let slots = storage.load(SLOT_COUNT);

        let device = BLEDevice::take();
        device
            .security()
            .set_auth(AuthReq::Bond | AuthReq::Sc)
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let (tx, rx) = mpsc::channel();
        let tx = Arc::new(StdMutex::new(tx));

        let server = device.get_server();
        // The state machine starts advertising explicitly, after the whitelist
        // has been configured for the selected slot.
        server.advertise_on_disconnect(false);

        {
            let tx = tx.clone();
            server.on_connect(move |_server, desc| {
                let (address, address_type) = peer_address(desc);
                send_event(
                    &tx,
                    BleEvent::Connected {
                        conn: desc.conn_handle(),
                        address,
                        address_type,
                    },
                );
            });
        }
        {
            let tx = tx.clone();
            server.on_disconnect(move |desc, _result| {
                send_event(
                    &tx,
                    BleEvent::Disconnected {
                        conn: desc.conn_handle(),
                    },
                );
            });
        }
        {
            let tx = tx.clone();
            server.on_authentication_complete(move |_server, desc, result| {
                let (address, address_type) = peer_address(desc);
                send_event(
                    &tx,
                    BleEvent::Authenticated {
                        conn: desc.conn_handle(),
                        ok: result.is_ok() && desc.encrypted(),
                        address,
                        address_type,
                    },
                );
            });
        }

        let mut hid = BLEHIDDevice::new(server);
        hid.report_map(REPORT_MAP);
        hid.manufacturer("DIY");
        // Signature 0x02 (USB-IF), vendor/product mirror the C prototype.
        hid.pnp(0x02, 0xffff, 0x0001, 0x0100);
        // HID Information flags: bit 0 remote wake, bit 1 normally connectable.
        hid.hid_info(0x00, 0x02);
        hid.set_battery_level(100);

        let keyboard = hid.input_report(KEYBOARD_REPORT_ID);
        let mouse = hid.input_report(MOUSE_REPORT_ID);
        let consumer = hid.input_report(CONSUMER_REPORT_ID);
        let output = hid.output_report(KEYBOARD_REPORT_ID);

        let advertising = device.get_advertising();
        advertising.lock().scan_response(false);
        let mut advertisement = BLEAdvertisementData::new();
        advertisement
            .name("BT Switch")
            .appearance(0x03c0)
            .add_service_uuid(BleUuid::from_uuid16(0x1812));
        advertising
            .lock()
            .set_data(&mut advertisement)
            .map_err(|err| anyhow!("cannot set advertising data: {err:?}"))?;
        device
            .set_white_list(&[])
            .map_err(|err| anyhow!("cannot clear whitelist: {err:?}"))?;

        let mut pairing = PairingWindow::default();
        pairing.open(now_ms, PAIRING_SECONDS);

        let selected = slots.selected();
        log::info!(
            "BLE ready: {} slots, selected slot {}",
            slots.count(),
            selected + 1
        );

        Ok(Self {
            device,
            server,
            keyboard,
            mouse,
            consumer,
            _output: output,
            events: rx,
            storage,
            slots,
            pairing,
            connected: false,
            conn: 0,
            secured: false,
            ready: false,
            ready_at_ms: 0,
            connect_at_ms: 0,
            security_requested: false,
            security_attempts: 0,
            next_security_ms: 0,
            advertising: false,
            advertising_dirty: true,
            next_advertise_ms: 0,
            disconnect_requested: false,
            next_disconnect_ms: 0,
            disconnect_deadline_ms: 0,
            forget_pending: false,
        })
    }

    /// Drive the connection/security/advertising state machine.
    pub fn poll(&mut self, now_ms: u64) {
        while let Ok(event) = self.events.try_recv() {
            self.handle_event(event, now_ms);
        }

        if self.connected {
            if self.disconnect_requested {
                if self.disconnect_deadline_ms == 0 {
                    self.disconnect_deadline_ms = now_ms.saturating_add(5_000);
                }
                if now_ms >= self.disconnect_deadline_ms {
                    // The controller did not deliver a disconnect event. Give
                    // up on this link so the state machine cannot deadlock.
                    log::warn!("disconnect timed out; forcing link teardown");
                    self.connected = false;
                    self.secured = false;
                    self.ready = false;
                    self.disconnect_requested = false;
                    self.disconnect_deadline_ms = 0;
                    self.advertising = false;
                    self.advertising_dirty = true;
                } else if now_ms >= self.next_disconnect_ms {
                    let result = self.server.disconnect(self.conn);
                    self.next_disconnect_ms = now_ms.saturating_add(SECURITY_RETRY_MS);
                    if let Err(err) = result {
                        log::warn!("disconnect request failed: {err:?}");
                    }
                }
            }

            if self.connected && !self.secured {
                if now_ms.saturating_sub(self.connect_at_ms) > AUTH_TIMEOUT_MS {
                    log::warn!("authentication timed out");
                    self.disconnect_requested = true;
                } else if !self.security_requested && now_ms >= self.next_security_ms {
                    self.request_security(now_ms);
                }
            } else if !self.ready && now_ms >= self.ready_at_ms {
                self.ready = true;
                log::info!("slot {} ready", self.slots.selected() + 1);
            }
        } else {
            if self.forget_pending {
                self.do_forget(now_ms);
            } else if self.slots.desired() != self.slots.selected() {
                self.slots.commit();
                if !self.slots.is_used(self.slots.selected()) {
                    self.pairing.open(now_ms, PAIRING_SECONDS);
                }
                self.advertising_dirty = true;
                self.next_advertise_ms = 0;
                self.save();
                log::info!("selected slot {}", self.slots.selected() + 1);
            }

            let allowed = self.advertising_allowed(now_ms);
            if self.advertising && (!allowed || self.advertising_dirty) {
                self.stop_advertising();
            }
            if !self.advertising && allowed && now_ms >= self.next_advertise_ms {
                self.start_advertising(now_ms);
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
            && self.connected
            && self.slots.selected() == self.slots.desired()
            && !self.forget_pending
    }

    pub fn is_advertising(&self) -> bool {
        self.advertising
    }

    pub fn slot_count(&self) -> usize {
        self.slots.count()
    }

    /// Switch to another host. Input stays disabled until the new host is
    /// encrypted and all keys/buttons have been released.
    pub fn select(&mut self, slot: usize, now_ms: u64) -> bool {
        if self.forget_pending || slot >= self.slots.count() || slot == self.slots.desired() {
            return false;
        }
        if self.is_ready() {
            self.release_reports();
        }
        self.ready = false;
        self.slots.select(slot);
        if self.connected {
            self.disconnect_requested = true;
            self.next_disconnect_ms = now_ms;
        }
        log::info!("switch requested: slot {}", slot + 1);
        true
    }

    pub fn next_slot(&mut self, now_ms: u64) -> bool {
        let next = (self.slots.desired() + 1) % self.slots.count();
        self.select(next, now_ms)
    }

    pub fn prev_slot(&mut self, now_ms: u64) -> bool {
        let previous = (self.slots.desired() + self.slots.count() - 1) % self.slots.count();
        self.select(previous, now_ms)
    }

    /// Open (or extend) the pairing window of an empty slot.
    pub fn pair(&mut self, now_ms: u64) {
        if self.slots.desired() != self.slots.selected() {
            log::warn!("wait for the slot switch to finish before pairing");
            return;
        }
        let slot = self.slots.selected();
        if self.slots.is_used(slot) {
            log::warn!("slot {} is already paired; forget it first", slot + 1);
            return;
        }
        self.pairing.open(now_ms, PAIRING_SECONDS);
        self.advertising_dirty = true;
        self.next_advertise_ms = 0;
        log::info!(
            "pairing open on slot {} for {} s",
            slot + 1,
            PAIRING_SECONDS
        );
    }

    /// Remove the host from the selected slot. The host must also forget the
    /// device in its own Bluetooth settings.
    pub fn forget(&mut self, now_ms: u64) {
        if self.slots.desired() != self.slots.selected() {
            log::warn!("wait for the slot switch to finish before forgetting");
            return;
        }
        if self.is_ready() {
            self.release_reports();
        }
        self.ready = false;
        self.forget_pending = true;
        if self.connected {
            self.disconnect_requested = true;
            self.next_disconnect_ms = now_ms;
        } else {
            self.do_forget(now_ms);
        }
    }

    pub fn status(&self, now_ms: u64) {
        log::info!(
            "slot {}/{} connected={} ready={} secured={} advertising={} pairing={}s",
            self.slots.desired() + 1,
            self.slots.count(),
            self.connected,
            self.is_ready(),
            self.secured,
            self.advertising,
            self.pairing.remaining_ms(now_ms) / 1000,
        );
        for (index, slot) in self.slots.slots().iter().enumerate() {
            if slot.used {
                log::info!("  {}: paired {}", index + 1, format_address(slot.address));
            } else {
                log::info!("  {}: empty", index + 1);
            }
        }
    }

    /// Send the release reports to the currently selected host.
    pub fn release_reports(&mut self) {
        if !self.is_ready() {
            return;
        }
        let _ = send_chr(&self.keyboard, &ZERO_KEYBOARD);
        let _ = send_chr(&self.mouse, &ZERO_MOUSE);
        let _ = send_chr(&self.consumer, &ZERO_CONSUMER);
    }

    /// Notify the reports produced by the translator. Returns which reports
    /// actually had a subscribed host and were therefore sent.
    pub fn send_reports(&mut self, reports: &Reports) -> Sent {
        let mut sent = Sent::default();
        if !self.is_ready() {
            return sent;
        }
        if let Some(data) = reports.keyboard {
            sent.keyboard = send_chr(&self.keyboard, &data);
        }
        if let Some(data) = reports.consumer {
            sent.consumer = send_chr(&self.consumer, &data);
        }
        if let Some(data) = reports.mouse {
            sent.mouse = send_chr(&self.mouse, &data);
        }
        sent
    }

    fn handle_event(&mut self, event: BleEvent, now_ms: u64) {
        match event {
            BleEvent::Connected {
                conn,
                address,
                address_type,
            } => {
                log::info!("host connected: {} conn={}", format_address(address), conn);
                self.connected = true;
                self.conn = conn;
                self.advertising = false;
                self.secured = false;
                self.ready = false;
                self.security_requested = false;
                self.security_attempts = 0;
                self.connect_at_ms = now_ms;
                self.next_security_ms = now_ms.saturating_add(100);
                self.disconnect_requested = false;
                self.disconnect_deadline_ms = 0;

                if self.slots.desired() != self.slots.selected()
                    || (!self.slots.is_used(self.slots.selected()) && !self.pairing.is_open(now_ms))
                {
                    log::warn!("rejecting host outside the selected/pairing slot");
                    self.disconnect_requested = true;
                    self.next_disconnect_ms = now_ms;
                } else {
                    log::debug!(
                        "host address {:?} type {}",
                        format_address(address),
                        address_type
                    );
                }
            }
            BleEvent::Disconnected { conn } => {
                if !self.connected || self.conn != conn {
                    return;
                }
                log::info!("host disconnected");
                self.connected = false;
                self.secured = false;
                self.ready = false;
                self.security_requested = false;
                self.disconnect_requested = false;
                self.disconnect_deadline_ms = 0;
                // The next poll commits a pending selection and restarts
                // advertising with the correct whitelist.
                self.advertising = false;
                self.advertising_dirty = true;
                self.next_advertise_ms = 0;
            }
            BleEvent::Authenticated {
                conn,
                ok,
                address,
                address_type,
            } => {
                if !self.connected || self.conn != conn {
                    return;
                }
                if !ok {
                    log::warn!("authentication failed; disconnecting");
                    self.disconnect_requested = true;
                    self.next_disconnect_ms = now_ms;
                    return;
                }
                if self.slots.desired() != self.slots.selected() {
                    log::warn!("authentication completed during a pending switch");
                    self.disconnect_requested = true;
                    self.next_disconnect_ms = now_ms;
                    return;
                }

                let slot = self.slots.selected();
                if self.slots.is_used(slot) {
                    if !self.slots.matches(slot, address) {
                        log::warn!(
                            "host {} does not belong to slot {}; rejecting",
                            format_address(address),
                            slot + 1
                        );
                        self.cleanup_rejected_bond(address, address_type);
                        self.disconnect_requested = true;
                        self.next_disconnect_ms = now_ms;
                        return;
                    }
                } else {
                    if !self.pairing.is_open(now_ms) {
                        log::warn!("pairing window is closed; rejecting host");
                        self.cleanup_rejected_bond(address, address_type);
                        self.disconnect_requested = true;
                        self.next_disconnect_ms = now_ms;
                        return;
                    }
                    match self.slots.bind(slot, address, address_type) {
                        Ok(()) => {
                            self.pairing.close();
                            self.save();
                            log::info!(
                                "paired host {} saved in slot {}",
                                format_address(address),
                                slot + 1
                            );
                        }
                        Err(BindError::Duplicate(other)) => {
                            log::warn!("host already belongs to slot {}; rejecting", other + 1);
                            self.disconnect_requested = true;
                            self.next_disconnect_ms = now_ms;
                            return;
                        }
                        Err(err) => {
                            log::error!("cannot bind host: {err:?}");
                            self.disconnect_requested = true;
                            self.next_disconnect_ms = now_ms;
                            return;
                        }
                    }
                }

                self.secured = true;
                self.ready = false;
                self.ready_at_ms = now_ms.saturating_add(READY_DELAY_MS);
                log::info!(
                    "slot {} encrypted; waiting for host HID subscription",
                    slot + 1
                );
            }
        }
    }

    fn request_security(&mut self, now_ms: u64) {
        let rc = unsafe { sys::ble_gap_security_initiate(self.conn) };
        if rc == 0 || rc == sys::BLE_HS_EALREADY as i32 {
            self.security_requested = true;
            return;
        }
        self.security_attempts = self.security_attempts.saturating_add(1);
        self.next_security_ms = now_ms.saturating_add(SECURITY_RETRY_MS);
        log::warn!(
            "security initiate failed: {} (attempt {}/{})",
            rc,
            self.security_attempts,
            SECURITY_ATTEMPTS
        );
        if self.security_attempts >= SECURITY_ATTEMPTS {
            // Stop retrying; the connection timeout will close the link.
            self.security_requested = true;
        }
    }

    fn advertising_allowed(&self, now_ms: u64) -> bool {
        !self.connected
            && !self.forget_pending
            && self.slots.desired() == self.slots.selected()
            && (self.slots.is_used(self.slots.selected()) || self.pairing.is_open(now_ms))
    }

    fn start_advertising(&mut self, now_ms: u64) {
        let Some(slot) = self.slots.slot(self.slots.selected()).copied() else {
            return;
        };

        let mut filter_connections = slot.used;
        if slot.used {
            let address = to_ble_address(slot.address, slot.address_type);
            if let Err(err) = self.device.set_white_list(&[address]) {
                log::warn!("cannot set whitelist: {err:?}; using software validation");
                filter_connections = false;
            }
        }
        if !filter_connections {
            let _ = self.device.set_white_list(&[]);
        }

        let advertising = self.device.get_advertising();
        let mut advertising = advertising.lock();
        advertising.filter_policy(if filter_connections {
            AdvFilterPolicy::Connect
        } else {
            AdvFilterPolicy::None
        });
        match advertising.start() {
            Ok(()) => {
                self.advertising = true;
                self.advertising_dirty = false;
                self.next_advertise_ms = 0;
                log::info!(
                    "advertising slot {} ({})",
                    self.slots.selected() + 1,
                    if slot.used {
                        "paired host only"
                    } else {
                        "PAIRING OPEN"
                    }
                );
            }
            Err(err) => {
                if advertising.is_advertising() {
                    // EALREADY / another controller state: advertising is
                    // already active, so treat it as success.
                    self.advertising = true;
                    self.advertising_dirty = false;
                    self.next_advertise_ms = 0;
                } else {
                    self.advertising = false;
                    self.next_advertise_ms = now_ms.saturating_add(1000);
                    log::warn!("cannot start advertising: {err:?}");
                }
            }
        }
    }

    fn stop_advertising(&mut self) {
        let advertising = self.device.get_advertising();
        let advertising = advertising.lock();
        match advertising.stop() {
            Ok(()) => self.advertising = false,
            Err(err) => {
                log::warn!("cannot stop advertising: {err:?}");
                self.advertising = advertising.is_advertising();
            }
        }
    }

    fn do_forget(&mut self, now_ms: u64) {
        let slot = self.slots.selected();
        if let Some(saved) = self.slots.slot(slot).copied()
            && saved.used
        {
            let address = to_ble_address(saved.address, saved.address_type);
            if let Err(err) = self.device.delete_bond(&address) {
                log::warn!("cannot delete bond: {err:?}");
            }
        }
        self.slots.forget(slot);
        self.forget_pending = false;
        self.pairing.open(now_ms, PAIRING_SECONDS);
        self.advertising_dirty = true;
        self.next_advertise_ms = 0;
        self.save();
        log::warn!(
            "forgot slot {}; remove 'BT Switch' from the host Bluetooth settings too",
            slot + 1
        );
    }

    fn cleanup_rejected_bond(&self, address: [u8; 6], address_type: u8) {
        // Never delete a bond that belongs to another configured slot.
        if self.slots.find(address).is_some() {
            return;
        }
        let address = to_ble_address(address, address_type);
        if let Err(err) = self.device.delete_bond(&address) {
            log::debug!("cannot delete rejected bond: {err:?}");
        }
    }

    fn save(&self) {
        self.storage.save(&self.slots);
    }
}

fn send_chr(characteristic: &Arc<Mutex<BLECharacteristic>>, data: &[u8]) -> bool {
    let mut characteristic = characteristic.lock();
    if characteristic.subscribed_count() == 0 {
        return false;
    }
    characteristic.set_value(data).notify();
    true
}

fn send_event(tx: &StdMutex<Sender<BleEvent>>, event: BleEvent) {
    if let Ok(tx) = tx.lock() {
        let _ = tx.send(event);
    }
}

fn peer_address(desc: &BLEConnDesc) -> ([u8; 6], u8) {
    let identity = desc.id_address();
    let address = if identity.as_le_bytes() == [0; 6] {
        desc.address()
    } else {
        identity
    };
    (address.as_le_bytes(), address.addr_type() as u8)
}

fn to_ble_address(address: [u8; 6], address_type: u8) -> BLEAddress {
    let address_type = BLEAddressType::try_from(address_type).unwrap_or(BLEAddressType::Public);
    BLEAddress::from_le_bytes(address, address_type)
}

fn format_address(address: [u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        address[5], address[4], address[3], address[2], address[1], address[0]
    )
}
