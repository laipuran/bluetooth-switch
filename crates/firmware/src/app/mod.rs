//! 应用调度：按顺序处理 BLE 管理、用户控制、USB 输入和报告发送。
//! 此模块不负责板级引脚、USB 描述符或 BLE 属性实现。

mod controls;
mod input;

use std::sync::mpsc;
use std::time::Instant;

use anyhow::Result;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::{ble, board, console, usb};
use controls::Buttons;
use input::InputPipeline;

pub fn run() -> Result<()> {
    let started = Instant::now();
    let now_ms = || started.elapsed().as_millis() as u64;
    let mut board = board::Board::new(Peripherals::take()?)?;

    // NVS 必须先初始化，BLE 持久化配对依赖它。
    let partition = EspDefaultNvsPartition::take()?;
    let usb = usb::start()?;
    let mut ble = ble::BleManager::new(partition, now_ms())?;
    let mut input = InputPipeline::new();
    let mut buttons = Buttons::new();

    let (command_tx, command_rx) = mpsc::channel();
    console::spawn(command_tx)?;
    console::print_help(ble.slot_count());

    loop {
        let now = now_ms();
        ble.poll(now);

        for command in command_rx.try_iter().take(8) {
            controls::apply(&mut ble, &mut input, &usb, command, now);
        }
        for command in buttons.poll(now, &board).into_iter().flatten() {
            controls::apply(&mut ble, &mut input, &usb, command, now);
        }

        input.poll(&usb, &mut ble);

        let green = ble.is_ready() || (ble.is_advertising() && (now / 500) % 2 == 0);
        board.set_leds(green, usb.attached());

        // 结构整理阶段保留原有 4 ms 调度，性能修改另行提交。
        FreeRtos::delay_ms(4);
    }
}
