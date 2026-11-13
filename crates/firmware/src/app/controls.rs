//! 将物理手势和串口指令归一为相同操作，集中维护切换时的输入保护。

use bt_switch_core::control::{Button, Gesture};

use super::input::InputPipeline;
use crate::ble::BleManager;
use crate::board::Board;
use crate::console::{self, Command};
use crate::usb::UsbInput;

pub(super) struct Buttons {
    ok: Button,
    up: Button,
    down: Button,
    menu: Button,
}

impl Buttons {
    pub(super) fn new() -> Self {
        Self {
            ok: Button::new(15, 3000),
            up: Button::new(15, 3000),
            down: Button::new(15, 3000),
            menu: Button::new(15, 5000),
        }
    }

    pub(super) fn poll(&mut self, now: u64, board: &Board) -> [Option<Command>; 4] {
        let ok = self
            .ok
            .update(now, board.ok_pressed())
            .map(|gesture| match gesture {
                Gesture::Click => {
                    log::info!("Button: short press -> next slot");
                    Command::Next
                }
                Gesture::LongPress => {
                    log::info!("Button: long press -> pairing");
                    Command::Pair
                }
            });
        let up = match self.up.update(now, board.up_pressed()) {
            Some(Gesture::Click) => Some(Command::Prev),
            _ => None,
        };
        let down = match self.down.update(now, board.down_pressed()) {
            Some(Gesture::Click) => Some(Command::Pair),
            _ => None,
        };
        let menu = self
            .menu
            .update(now, board.menu_pressed())
            .map(|gesture| match gesture {
                Gesture::Click => Command::Status,
                Gesture::LongPress => Command::Forget,
            });
        [ok, up, down, menu]
    }
}

pub(super) fn apply(
    ble: &mut BleManager,
    input: &mut InputPipeline,
    usb: &UsbInput,
    command: Command,
    now: u64,
) {
    let changed = match command {
        Command::Next => ble.next_slot(now),
        Command::Prev => ble.prev_slot(now),
        Command::Select(slot) => ble.select(slot, now),
        Command::Forget => {
            ble.forget(now);
            true
        }
        Command::Pair => {
            ble.pair(now);
            false
        }
        Command::Status => {
            ble.status(now);
            usb.status();
            false
        }
        Command::Probe => {
            // 仅发送释放报告，不合成按键、位移或滚轮。
            ble.release_reports();
            false
        }
        Command::UsbRescan => {
            usb.rescan();
            false
        }
        Command::Help => {
            console::print_help(ble.slot_count());
            false
        }
    };
    if changed {
        input.disarm();
    }
}
