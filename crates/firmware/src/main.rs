//! 固件入口；应用调度、硬件适配和纯逻辑分别维护。

mod app;
mod ble;
mod board;
mod console;
mod storage;
mod usb;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // 每次通知两行 INFO 会阻塞串口；保留 NimBLE 警告、错误和应用状态日志。
    unsafe {
        esp_idf_svc::sys::esp_log_level_set(
            c"NimBLE".as_ptr(),
            esp_idf_svc::sys::esp_log_level_t_ESP_LOG_WARN,
        );
    }
    log::info!("BT Switch (Rust) starting");
    app::run()
}
