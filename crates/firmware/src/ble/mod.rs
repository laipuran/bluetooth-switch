//! BLE 应用入口：在线集合与输入目标独立，切槽不拆除其他连接。

mod admission;
mod advertising;
mod attributes;
mod connection;
mod events;
mod management;
mod profile;
mod transport;

use std::sync::mpsc::{self, Receiver};

use anyhow::Result;
use bt_switch_core::Router;
use bt_switch_core::report::{Reports, Sent};
use bt_switch_core::slots::{PairingWindow, SlotTable};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp32_nimble::enums::{AuthReq, SecurityIOCap};
use esp32_nimble::{BLEDevice, BLEServer};

use advertising::Advertising;
use attributes::Sessions;
use connection::Connection;
use events::BleEvent;
use profile::HidProfile;
use transport::HidTransport;

use crate::storage::Storage;

/// 槽位数必须同时满足 NimBLE 连接数及控制器活动数配置。
pub const SLOT_COUNT: usize = 3;
const PAIRING_SECONDS: u64 = 120;

pub struct BleManager {
    device: &'static mut BLEDevice,
    server: &'static mut BLEServer,
    transport: HidTransport,
    events: Receiver<BleEvent>,
    storage: Storage,
    slots: SlotTable,
    pairing: PairingWindow,
    pairing_slot: Option<usize>,
    connections: Vec<Connection>,
    router: Router,
    advertising: Advertising,
    forget_pending: Option<usize>,
    forget_retry_ms: u64,
    input_reset: bool,
}

impl BleManager {
    pub fn new(partition: EspDefaultNvsPartition, now_ms: u64) -> Result<Self> {
        assert!(esp_idf_svc::sys::CONFIG_BT_NIMBLE_MAX_CONNECTIONS as usize >= SLOT_COUNT);
        assert!(esp_idf_svc::sys::CONFIG_BT_CTRL_BLE_MAX_ACT as usize > SLOT_COUNT);
        let storage = Storage::new(partition)?;
        let slots = storage.load(SLOT_COUNT);
        let router = Router::new(slots.count(), slots.selected());

        let device = BLEDevice::take();
        device
            .security()
            .set_auth(AuthReq::Bond | AuthReq::Sc)
            .set_io_cap(SecurityIOCap::NoInputNoOutput)
            .resolve_rpa();

        let (tx, rx) = mpsc::channel();
        let sessions = Sessions::default();
        let server = device.get_server();
        events::register(server, tx.clone(), sessions.clone());
        let profile = HidProfile::new(server);
        let transport = HidTransport::new(profile, sessions, tx);
        let advertising = Advertising::new(device)?;

        let mut manager = Self {
            device,
            server,
            transport,
            events: rx,
            storage,
            slots,
            router,
            pairing: PairingWindow::default(),
            pairing_slot: None,
            connections: Vec::with_capacity(SLOT_COUNT),
            advertising,
            forget_pending: None,
            forget_retry_ms: 0,
            input_reset: true,
        };
        if !manager.slots.is_used(manager.slots.selected()) {
            manager.open_pairing(manager.slots.selected(), now_ms);
        }
        log::info!(
            "BLE multilink ready: {SLOT_COUNT} slots, selected={}",
            manager.slots.selected() + 1
        );
        Ok(manager)
    }

    pub fn poll(&mut self, now_ms: u64) {
        while let Ok(event) = self.events.try_recv() {
            self.handle_event(event, now_ms);
        }

        // 新连接的安全请求串行发起；已安全连接不会因切槽反复握手。
        let mut security_busy = self.connections.iter().any(Connection::security_pending);
        for connection in &mut self.connections {
            connection.poll(self.server, !security_busy, now_ms);
            security_busy |= connection.security_pending();
        }
        self.finish_forget(now_ms);
        self.flush_releases();

        let pending_unknown = self.connections.iter().any(|link| !link.secured);
        let pairing = self.pairing_open(now_ms) && !pending_unknown;
        let known = self.slots.slots().iter().any(|slot| slot.used);
        let allowed = self.connections.len() < SLOT_COUNT && (known || pairing);
        self.advertising
            .poll(self.device, allowed, &self.slots, pairing, now_ms);
    }

    pub fn is_ready(&self) -> bool {
        self.forget_pending != Some(self.slots.selected()) && self.router.ready()
    }

    /// 连接/订阅变化可能在一次 poll 内发生；仍要求输入层重新等待物理释放。
    pub fn take_input_reset(&mut self) -> bool {
        std::mem::take(&mut self.input_reset)
    }

    pub fn is_advertising(&self) -> bool {
        self.advertising.is_active()
    }

    pub fn slot_count(&self) -> usize {
        self.slots.count()
    }

    pub fn release_reports(&mut self) {
        self.router.release_target();
        self.flush_releases();
    }

    pub fn send_reports(&mut self, reports: &Reports) -> Sent {
        let mut sent = Sent::default();
        if !self.is_ready() {
            return sent;
        }

        let transport = &self.transport;
        let mut send = |report, data: &[u8]| {
            self.router.send(report, data, |conn, report, data| {
                transport.notify(conn, report, data)
            })
        };
        if let Some(data) = reports.keyboard {
            sent.keyboard = send(1, &data);
        }
        if let Some(data) = reports.mouse {
            sent.mouse = send(2, &data);
        }
        if let Some(data) = reports.consumer {
            sent.consumer = send(3, &data);
        }
        sent
    }

    fn flush_releases(&mut self) {
        let transport = &self.transport;
        self.router
            .flush(|conn, report, data| transport.notify(conn, report, data));
    }

    fn pairing_open(&self, now_ms: u64) -> bool {
        self.pairing.is_open(now_ms)
            && self
                .pairing_slot
                .is_some_and(|slot| !self.slots.is_used(slot) && self.forget_pending != Some(slot))
    }

    fn connection_mut(&mut self, conn: u16) -> Option<&mut Connection> {
        self.connections.iter_mut().find(|link| link.handle == conn)
    }
}
