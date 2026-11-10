//! Board-specific wiring for the Espressif ESP32-S3-USB-OTG devkit.
//!
//! The USB Type-A "HOST" socket is shared with the ESP32-S3 native USB pins
//! through a mux:
//!
//! * GPIO18 (`USB_SEL`) high -> GPIO19/20 go to the Type-A host socket.
//! * GPIO12 (`DEV_VBUS_EN`) high -> host socket is powered from the USB_DEV
//!   port (the Type-A male plug / a powered hub).
//! * GPIO13 (`BOOST_EN`) high -> host socket is powered from the battery
//!   boost converter instead.
//! * GPIO17 (`LIMIT_EN`) high -> enable the 500 mA current limiter.
//!
//! Because USB_SEL is high, the native USB port is *not* available for
//! flashing. Use the Micro-USB UART connector for flashing and the console.
//!
//! If you use a custom board, change the pin constants and the power source
//! below. GPIO19/20 are the USB data pins and must never be driven as GPIOs.

use anyhow::Result;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;

/// Where the USB-A host socket gets its 5 V from.
#[allow(dead_code)] // selected by the HOST_POWER constant below
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPower {
    /// Power from the USB_DEV connector (GPIO12 high, GPIO13 low).
    DevVbus,
    /// Power from the battery boost converter (GPIO13 high, GPIO12 low).
    BatteryBoost,
    /// Leave the power pins alone (board is powered externally).
    External,
}

/// Default power source. Change this when running from a LiPo.
const HOST_POWER: HostPower = HostPower::DevVbus;

pub struct Board<'d> {
    _usb_sel: PinDriver<'d, Output>,
    _limit_en: PinDriver<'d, Output>,
    _dev_vbus_en: PinDriver<'d, Output>,
    _boost_en: PinDriver<'d, Output>,
    led_green: PinDriver<'d, Output>,
    led_yellow: PinDriver<'d, Output>,
    btn_ok: PinDriver<'d, Input>,
    btn_up: PinDriver<'d, Input>,
    btn_down: PinDriver<'d, Input>,
    btn_menu: PinDriver<'d, Input>,
}

impl<'d> Board<'d> {
    pub fn new(peripherals: Peripherals) -> Result<Self> {
        let pins = peripherals.pins;

        let mut usb_sel = PinDriver::output(pins.gpio18)?;
        let mut limit_en = PinDriver::output(pins.gpio17)?;
        let mut dev_vbus_en = PinDriver::output(pins.gpio12)?;
        let mut boost_en = PinDriver::output(pins.gpio13)?;

        // Route the S3 USB pins to the Type-A host socket first.
        usb_sel.set_high()?;
        limit_en.set_high()?;

        match HOST_POWER {
            HostPower::DevVbus => {
                boost_en.set_low()?;
                dev_vbus_en.set_high()?;
            }
            HostPower::BatteryBoost => {
                dev_vbus_en.set_low()?;
                boost_en.set_high()?;
            }
            HostPower::External => {
                dev_vbus_en.set_low()?;
                boost_en.set_low()?;
            }
        }

        let mut led_green = PinDriver::output(pins.gpio15)?;
        let mut led_yellow = PinDriver::output(pins.gpio16)?;
        led_green.set_low()?;
        led_yellow.set_low()?;

        let btn_ok = PinDriver::input(pins.gpio0, Pull::Up)?;
        let btn_up = PinDriver::input(pins.gpio10, Pull::Up)?;
        let btn_down = PinDriver::input(pins.gpio11, Pull::Up)?;
        let btn_menu = PinDriver::input(pins.gpio14, Pull::Up)?;

        // Give the 2.4 GHz receiver time to power up before USB enumeration.
        FreeRtos::delay_ms(100);

        Ok(Self {
            _usb_sel: usb_sel,
            _limit_en: limit_en,
            _dev_vbus_en: dev_vbus_en,
            _boost_en: boost_en,
            led_green,
            led_yellow,
            btn_ok,
            btn_up,
            btn_down,
            btn_menu,
        })
    }

    /// Green LED: selected host is ready.
    /// Yellow LED: at least one USB HID interface is active.
    pub fn set_leds(&mut self, green: bool, yellow: bool) {
        let _ = if green {
            self.led_green.set_high()
        } else {
            self.led_green.set_low()
        };
        let _ = if yellow {
            self.led_yellow.set_high()
        } else {
            self.led_yellow.set_low()
        };
    }

    pub fn ok_pressed(&self) -> bool {
        self.btn_ok.is_low()
    }

    pub fn up_pressed(&self) -> bool {
        self.btn_up.is_low()
    }

    pub fn down_pressed(&self) -> bool {
        self.btn_down.is_low()
    }

    pub fn menu_pressed(&self) -> bool {
        self.btn_menu.is_low()
    }
}
