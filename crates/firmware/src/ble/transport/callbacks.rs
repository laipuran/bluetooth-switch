//! 动态 HID 属性的逐连接读写及 CCCD 回调。

use std::sync::mpsc::Sender;

use bt_switch_core::session::boot_mouse;
use esp32_nimble::NimbleSub;

use crate::ble::attributes::Sessions;
use crate::ble::events::BleEvent;
use crate::ble::profile::{Characteristic, HidProfile};

pub(super) fn register(profile: &HidProfile, sessions: &Sessions, tx: &Sender<BleEvent>) {
    for (index, input) in profile.inputs.iter().enumerate() {
        input_callbacks(input, index as u8 + 1, false, sessions, tx);
    }
    input_callbacks(&profile.boot_keyboard, 1, true, sessions, tx);
    input_callbacks(&profile.boot_mouse, 2, true, sessions, tx);
    protocol_callbacks(profile, sessions, tx);
    output_callbacks(profile, sessions);
}

fn input_callbacks(
    chr: &Characteristic,
    report: u8,
    boot: bool,
    sessions: &Sessions,
    tx: &Sender<BleEvent>,
) {
    let values = sessions.clone();
    chr.lock().on_read(move |chr, desc| {
        let session = values.get(desc.conn_handle()).unwrap_or_default();
        if boot && report == 2 {
            chr.set_value(&boot_mouse(session.value(report)).unwrap_or([0; 3]));
        } else {
            chr.set_value(session.value(report));
        }
    });

    let values = sessions.clone();
    let tx = tx.clone();
    chr.lock().on_subscribe(move |_, desc, flags| {
        values.update(desc.conn_handle(), |state| {
            state.subscribe(report, boot, flags.contains(NimbleSub::NOTIFY))
        });
        let _ = tx.send(BleEvent::SessionChanged {
            conn: desc.conn_handle(),
            release: false,
        });
    });
}

fn protocol_callbacks(profile: &HidProfile, sessions: &Sessions, tx: &Sender<BleEvent>) {
    let values = sessions.clone();
    profile.protocol.lock().on_read(move |chr, desc| {
        chr.set_value(&[values.get(desc.conn_handle()).unwrap_or_default().protocol]);
    });

    for (chr, protocol) in [(&profile.protocol, true), (&profile.control, false)] {
        let values = sessions.clone();
        let tx = tx.clone();
        chr.lock().on_write(move |args| {
            if args.recv_data().len() != 1 || args.recv_data()[0] > 1 {
                args.reject_with_error_code(0x0d);
                return;
            }
            let value = args.recv_data()[0];
            let conn = args.desc().conn_handle();
            if !values.update(conn, |state| {
                if protocol {
                    state.protocol = value;
                } else {
                    state.suspended = value == 0;
                }
            }) {
                args.reject();
                return;
            }
            let _ = tx.send(BleEvent::SessionChanged {
                conn,
                release: true,
            });
        });
    }
}

fn output_callbacks(profile: &HidProfile, sessions: &Sessions) {
    for output in &profile.outputs {
        let values = sessions.clone();
        output.lock().on_read(move |chr, desc| {
            chr.set_value(&[values.get(desc.conn_handle()).unwrap_or_default().leds]);
        });

        let values = sessions.clone();
        output.lock().on_write(move |args| {
            if args.recv_data().len() != 1 {
                args.reject_with_error_code(0x0d);
                return;
            }
            let leds = args.recv_data()[0];
            if !values.update(args.desc().conn_handle(), |state| state.leds = leds) {
                args.reject();
            }
        });
    }
}
