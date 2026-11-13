//! 只读连接参数快照；不发起参数更新、不改变主机的协商策略。

use esp_idf_svc::sys;

pub(super) fn status(connection: u16) {
    let mut desc = sys::ble_gap_conn_desc::default();
    let error = unsafe { sys::ble_gap_conn_find(connection, &mut desc) };
    if error == 0 {
        log::info!(
            "BLE link: conn={} interval_us={} latency={} supervision_timeout_ms={}",
            connection,
            u32::from(desc.conn_itvl) * 1250,
            desc.conn_latency,
            u32::from(desc.supervision_timeout) * 10
        );
    } else {
        log::warn!("BLE link snapshot conn={connection} failed: {error}");
    }
}
