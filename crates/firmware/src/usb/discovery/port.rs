//! 只读控制器寄存器快照；端口 power 位只是控制命令，不是外部 5 V 的测量。

use esp_idf_svc::sys::{self, usb_hid as usb};

pub(super) fn status() {
    // 仅在 USB Host 库完成 PHY 初始化后调用，不写寄存器或改动 GPIO。
    let (port, interrupts) = unsafe {
        (
            std::ptr::addr_of!(usb::USB_DWC.hprt_reg.val).read_volatile(),
            std::ptr::addr_of!(usb::USB_DWC.gintsts_reg.val).read_volatile(),
        )
    };
    log::info!(
        "USB port: host_mode={} connected={} enabled={} line_state={} power_command={} overcurrent={} HPRT={:#010x}",
        interrupts & 1 != 0,
        port & 1 != 0,
        port & 4 != 0,
        (port >> 10) & 3,
        port & (1 << 12) != 0,
        port & (1 << 4) != 0,
        port
    );
    let (wrap, rtc) = unsafe {
        (
            std::ptr::addr_of!(usb::USB_WRAP.otg_conf.val).read_volatile(),
            std::ptr::addr_of!(sys::RTCCNTL.usb_conf.val).read_volatile(),
        )
    };
    // ESP32-S3：软件选择打开且 sw_usb_phy_sel=1 时，内部 PHY 属于 OTG 而非 USB-Serial/JTAG。
    // 只读这些选择位；不通过改写引脚或强制阈值来掩盖物理线路问题。
    log::info!(
        "USB PHY: external={} pads_enabled={} mux_software={} internal_to_otg={} pull_override={} dp_pd={} dm_pd={} rtc_pad_override={} rtc_pads_enabled={} WRAP={:#010x} RTC={:#010x}",
        wrap & (1 << 2) != 0,
        wrap & (1 << 18) != 0,
        rtc & (1 << 20) != 0,
        rtc & (1 << 19) != 0,
        wrap & (1 << 12) != 0,
        wrap & (1 << 14) != 0,
        wrap & (1 << 16) != 0,
        rtc & (1 << 11) != 0,
        rtc & (1 << 12) != 0,
        wrap,
        rtc
    );
}
