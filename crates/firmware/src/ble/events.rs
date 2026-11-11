//! 将 NimBLE 回调转换为应用任务事件；回调不直接修改管理器状态。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use esp32_nimble::{BLEAddress, BLEAddressType, BLEConnDesc, BLEServer};

use super::attributes::Sessions;

#[derive(Debug)]
pub(super) enum BleEvent {
    Connected {
        conn: u16,
        address: [u8; 6],
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
    SessionChanged {
        conn: u16,
        release: bool,
    },
}

pub(super) fn register(server: &mut BLEServer, tx: Sender<BleEvent>, sessions: Sessions) {
    let tx = Arc::new(Mutex::new(tx));

    // 广播由应用任务统一控制，避免回调与白名单更新竞争。
    server.advertise_on_disconnect(false);

    let connected_tx = tx.clone();
    let connected_sessions = sessions.clone();
    server.on_connect(move |_server, desc| {
        // 回调内先建立属性快照，后续 ATT 请求不会等应用任务轮询。
        connected_sessions.connect(desc.conn_handle());
        let (address, _) = peer_address(desc);
        send_event(
            &connected_tx,
            BleEvent::Connected {
                conn: desc.conn_handle(),
                address,
            },
        );
    });

    let disconnected_tx = tx.clone();
    let disconnected_sessions = sessions.clone();
    server.on_disconnect(move |desc, _result| {
        disconnected_sessions.disconnect(desc.conn_handle());
        send_event(
            &disconnected_tx,
            BleEvent::Disconnected {
                conn: desc.conn_handle(),
            },
        );
    });

    server.on_authentication_complete(move |_server, desc, result| {
        sessions.update(desc.conn_handle(), |state| {
            state.encrypted = result.is_ok() && desc.encrypted()
        });
        if let Err(error) = &result {
            log::warn!(
                "authentication conn={} failed: {error:?}",
                desc.conn_handle()
            );
        }
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

fn send_event(tx: &Mutex<Sender<BleEvent>>, event: BleEvent) {
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

pub(super) fn to_ble_address(address: [u8; 6], address_type: u8) -> BLEAddress {
    let address_type = BLEAddressType::try_from(address_type).unwrap_or(BLEAddressType::Public);
    BLEAddress::from_le_bytes(address, address_type)
}

pub(super) fn format_address(address: [u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        address[5], address[4], address[3], address[2], address[1], address[0]
    )
}
