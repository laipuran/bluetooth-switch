//! 第三方板的单键原型；仅配置用户确认接线的 GPIO4，不操作 USB 供电和路由。

use anyhow::Result;
use esp_idf_svc::hal::gpio::{Input, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;

pub struct Board<'d> {
    button: PinDriver<'d, Input>,
}

impl<'d> Board<'d> {
    pub fn new(peripherals: Peripherals) -> Result<Self> {
        let button = PinDriver::input(peripherals.pins.gpio4, Pull::Up)?;
        log::info!(
            "GPIO4 button enabled: active-low, 15ms debounce; click=next slot, hold 3s=pair; initial_pressed={}",
            button.is_low()
        );
        Ok(Self { button })
    }

    pub fn ok_pressed(&self) -> bool {
        self.button.is_low()
    }

    // 单键原型没有额外按键或指示灯，避免触碰其他引脚。
    pub fn set_leds(&mut self, _green: bool, _yellow: bool) {}

    pub fn up_pressed(&self) -> bool {
        false
    }

    pub fn down_pressed(&self) -> bool {
        false
    }

    pub fn menu_pressed(&self) -> bool {
        false
    }
}
