//! BT Switch: USB HID receiver -> multi-host Bluetooth HID switcher.
//!
//! 默认使用第三方 ESP32-S3 安全板级配置，不驱动未确认的 GPIO。
//! See README.md for wiring, controls and build instructions.

mod ble;
mod board;
mod console;
mod storage;
mod usb;

use std::sync::mpsc;
use std::time::Instant;

use anyhow::Result;
use bt_switch_core::control::{Button, Gesture};
use bt_switch_core::report::Translator;
use console::Command;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use usb::UsbEvent;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("BT Switch (Rust) starting");

    let started = Instant::now();
    let now_ms = || started.elapsed().as_millis() as u64;

    let peripherals = Peripherals::take()?;
    let mut board = board::Board::new(peripherals)?;

    // NVS first: ESP-IDF and NimBLE bond storage both use it.
    let partition = EspDefaultNvsPartition::take()?;
    let usb = usb::start()?;
    let mut ble = ble::BleManager::new(partition, now_ms())?;
    let mut translator = Translator::new();

    let (command_tx, command_rx) = mpsc::channel();
    console::spawn(command_tx)?;

    // One gesture detector per physical button.
    let mut button_ok = Button::new(15, 3000); // click: next, hold: pair
    let mut button_up = Button::new(15, 3000); // click: previous
    let mut button_down = Button::new(15, 3000); // click: open pairing
    let mut button_menu = Button::new(15, 5000); // click: status, hold: forget

    print_help(ble.slot_count());

    loop {
        let now = now_ms();
        ble.poll(now);

        for command in command_rx.try_iter().take(8) {
            apply_command(&mut ble, &mut translator, command, now, &usb);
        }

        if let Some(gesture) = button_ok.update(now, board.ok_pressed()) {
            match gesture {
                Gesture::Click => {
                    if ble.next_slot(now) {
                        translator.disarm();
                    }
                }
                Gesture::LongPress => ble.pair(now),
            }
        }
        if let Some(Gesture::Click) = button_up.update(now, board.up_pressed())
            && ble.prev_slot(now)
        {
            translator.disarm();
        }
        if let Some(Gesture::Click) = button_down.update(now, board.down_pressed()) {
            ble.pair(now);
        }
        if let Some(gesture) = button_menu.update(now, board.menu_pressed()) {
            match gesture {
                Gesture::Click => {
                    ble.status(now);
                    usb.status();
                }
                Gesture::LongPress => {
                    ble.forget(now);
                    translator.disarm();
                }
            }
        }

        while let Ok(event) = usb.events.try_recv() {
            match event {
                UsbEvent::Input { source, state } => {
                    translator.update(usize::from(source), &state);
                }
                UsbEvent::Detached { source } => {
                    translator.detach(usize::from(source));
                }
            }
        }
        if usb.take_overflow() {
            log::warn!("USB input queue overflow: released all keys");
            translator.reset();
            ble.release_reports();
            // The C prototype discards everything queued after an overflow;
            // partial state is worse than a short gap in input.
            while usb.events.try_recv().is_ok() {}
        }

        if ble.take_input_reset() {
            translator.disarm();
        }
        translator.set_ready(ble.is_ready());
        let reports = translator.flush();
        let sent = ble.send_reports(&reports);
        translator.confirm(&reports, sent);

        let green = ble.is_ready() || (ble.is_advertising() && (now / 500) % 2 == 0);
        board.set_leds(green, usb.attached());

        FreeRtos::delay_ms(4);
    }
}

fn apply_command(
    ble: &mut ble::BleManager,
    translator: &mut Translator,
    command: Command,
    now: u64,
    usb: &usb::UsbInput,
) {
    match command {
        Command::Next => {
            if ble.next_slot(now) {
                translator.disarm();
            }
        }
        Command::Prev => {
            if ble.prev_slot(now) {
                translator.disarm();
            }
        }
        Command::Select(slot) => {
            if ble.select(slot, now) {
                translator.disarm();
            }
        }
        Command::Pair => ble.pair(now),
        Command::Forget => {
            ble.forget(now);
            translator.disarm();
        }
        Command::Status => {
            ble.status(now);
            usb.status();
        }
        // 安全传输探针只发送释放状态，不合成按键、鼠标移动或滚轮。
        Command::Probe => ble.release_reports(),
        Command::Help => print_help(ble.slot_count()),
    }
}

fn print_help(slot_count: usize) {
    println!();
    println!("BT Switch (Rust) controls");
    println!(
        "  serial: n=next, b=previous, 1..{slot_count}=select slot, p=pair, X=forget, s=status, h=help"
    );
    println!("  t=send neutral HID probe to the selected host (no keys or movement)");
    #[cfg(feature = "official-otg")]
    {
        println!("  OK button: click=next slot, hold 3 s=pairing window");
        println!("  UP button: click=previous slot");
        println!("  DW button: click=pairing window");
        println!("  MENU button: click=status, hold 5 s=forget selected slot");
        println!(
            "  Green LED: ready solid / advertising blinking; yellow LED: USB receiver attached"
        );
    }

    #[cfg(not(feature = "official-otg"))]
    println!(
        "  Generic board: buttons/LEDs disabled; USB Host requires external VBUS verification"
    );
    println!();
}
