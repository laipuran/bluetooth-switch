//! 第三方板安全配置：不猜测 USB 供电、路由、LED 或按键引脚。
//!
//! 原生 USB D−/D+ 仍由 USB Host 驱动接管 GPIO19/20；VBUS 须由板级硬件供电。
//! 在确认原理图前，所有管理操作通过串口完成。

use anyhow::Result;
use esp_idf_svc::hal::peripherals::Peripherals;

pub struct Board;

impl Board {
    pub fn new(_peripherals: Peripherals) -> Result<Self> {
        log::warn!(
            "Generic board: GPIO controls disabled; use serial commands; verify Host VBUS externally"
        );
        Ok(Self)
    }

    pub fn set_leds(&mut self, _green: bool, _yellow: bool) {}

    pub fn ok_pressed(&self) -> bool {
        false
    }

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
