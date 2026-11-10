# 设计说明

## 1. 数据通路

```text
USB 接收器
  │  USB interrupt IN
  ▼
usb_host_hid（ESP-IDF C 组件）
  │  原始 report + report ID
  ▼
usb.rs / InterfaceCtx
  │  Parser::decode -> 每个 report ID 的 Decoded
  │  合并同一 interface 的所有 report ID
  ▼
mpsc channel（有界，128）
  ▼
main.rs
  │  Translator::update（按 interface 合并键盘/按钮/多媒体）
  ▼
Translator::flush
  │  keyboard / mouse / consumer 三个 report
  ▼
ble.rs
  │  NimBLE notify
  ▼
当前选中的 BLE 主机
```

### 为什么按 report ID 分别保存状态

一个 HID interface 可能用不同的 report ID 分别发送键盘、鼠标和 Consumer 数据。C 版本的 `usb_input.c` 为每个 report layout 保存一份 `hid_input_t`，每次收到数据后把它们合并，再把“当前完整状态 + 本次相对位移”交给主循环。Rust 版保持同样做法：

* `states[index]` 保存每个 layout 的按键/按钮/多媒体键。
* `merged.merge_state()` 对按键位图、鼠标按钮和 Consumer 做并集。
* 相对位移（X/Y/Wheel/Pan）只取当前数据包，绝不从 `states` 重放。

这样键盘 report 和 Consumer report 交替到达时不会互相清空状态。

## 2. 切换时的输入保护（armed）

切换主机时最危险的是把“按住的 Ctrl/Alt/鼠标键”带到新主机。`Translator` 有一个显式的 armed 状态：

```text
BLE not ready ──> disarm
                     │
BLE ready ──> 等待所有 source 的 keys/buttons/consumer 归零
                     │
                     ▼
             发送三份全零 report
                     │
             三份都成功 notify
                     ▼
                  armed
```

* 未 armed 时只更新 source 状态，不累加相对位移。
* armed 后 `flush()` 只在键盘/多媒体状态变化时发送；鼠标位移按主循环 tick 聚合。
* `send_reports()` 返回 `Sent`，`confirm()` 只把真正发送成功的 report 记为“已发送”。未发送的位移会保留到下一个 tick 重试，避免主机刚订阅 CCCD 时丢状态。
* 切换/忘记/断线时 `disarm()`，并把旧主机的三份 report 清零。

## 3. BLE 状态机

```text
                +-------------------+
                |  advertising      |
                |  白名单=当前槽位  |
                +---------+---------+
                          │ connect
                          ▼
                +-------------------+
                |  connected        |
                |  等待加密         |── 30 s 超时 ──> disconnect
                +---------+---------+
                          │ auth ok
                          ▼
                +-------------------+
                |  secured          |
                |  等 800 ms 让主机 |
                |  订阅 HID report  |
                +---------+---------+
                          │
                          ▼
                +-------------------+
                |  ready            |── 键鼠转发
                +-------------------+
```

* **连接校验**：连接时若 `desired != selected`，或槽位为空且配对窗口已关闭，立即断开。
* **认证**：连接后约 100 ms 主动调用 `ble_gap_security_initiate`；NimBLE 也会在主机订阅加密 characteristic 时自动发起。失败重试 3 次，30 s 未加密则断开。
* **绑定**：认证成功且槽位为空、配对窗口开启时，把主机 identity address 写入槽位并持久化；已占用槽位必须地址匹配。
* **拒绝清理**：认证成功但地址不属于当前槽位时，只有在地址不属于任何已配置槽位时才删除 bond，避免误删其他槽位的合法配对。
* **广播**：已配对槽位用单地址白名单 + `AdvFilterPolicy::Connect`；空槽位在配对窗口内使用无过滤广播。白名单设置失败时退化为无过滤广播，但仍然在认证回调里做地址校验。
* **断开兜底**：请求断开后 5 s 内没收到断开事件，就强制把状态复位并重新广播，防止 BLE 状态卡死。

## 4. 槽位持久化

NVS namespace `bt_switch`，key `slots`，固定 53 字节：

| 偏移 | 长度 | 内容 |
| --- | --- | --- |
| 0 | 4 | version = 1 (LE) |
| 4 | 1 | selected slot |
| 5 + 8·i | 1 | used |
| 6 + 8·i | 1 | BLE address type |
| 7 + 8·i | 6 | address (NimBLE little-endian) |

最多 6 个槽位（`bt_switch_core::MAX_SLOTS`），固件默认使用 3 个。任何字段不一致都会回退到空槽位表，而不是擦除整个 NVS。

## 5. 按键

`control::Button` 是一个纯逻辑、按 tick 驱动的消抖器：

* 原始电平变化后稳定 15 ms 才接受。
* 按下后持续 `long_ms` 发一次 `LongPress`。
* 释放时若未触发长按，则发 `Click`。
* 长按触发后，释放不再产生 Click。

默认映射见 README。要换成旋钮/编码器时，只需要把 `main.rs` 里的 `Button` 替换成编码器状态机，产生同样的 `Command`。

## 6. 线程模型

| 线程/任务 | 来源 | 职责 |
| --- | --- | --- |
| `main` | esp-idf-svc | 状态机 tick、按键、USB 事件消费、report 转发 |
| `usb_lib` | `std::thread` | `usb_host_lib_handle_events`、设备释放 |
| `usb_connect` | `std::thread` | HID interface open/start/close、描述符解析、上下文注册 |
| HID 后台任务 | `hid_host_install(create_background_task=true)` | 枚举/input report 回调，只做解码和非阻塞投递 |
| NimBLE host task | ESP-IDF | GAP/GATT/安全回调 |
| `console` | `std::thread` | 阻塞读取 UART 命令 |

共享状态：

* `TX`：USB 回调 -> 主循环的有界 channel。
* `LIFECYCLE_TX`：HID driver -> `usb_connect` 线程的连接/断开事件。
* `INTERFACES`：`Mutex<Vec<InterfaceCtx>>`，`usb_connect` 线程注册/移除，HID 回调解码。
* `OVERFLOW`：channel 满时置位；主循环清空 Translator 并发送全零 report。
* BLE 回调只通过 channel 发送事件，不直接碰主循环状态。

## 7. 板级细节

ESP32-S3-USB-OTG 的 USB 口经过一个模拟开关：

* `USB_SEL=1` 时，GPIO19/20 接 Type-A HOST 口；`USB_SEL=0` 时接 USB_DEV 口。
* 固件默认 `DEV_VBUS_EN=1`、`BOOST_EN=0`，从 USB_DEV 取 5 V 给 HOST 口。
* `LIMIT_EN=1` 启用 500 mA 限流。

因此烧录必须走 Micro-USB UART。若使用电池，把 `HOST_POWER` 改为 `BatteryBoost`。

## 8. 失败处理

| 情况 | 处理 |
| --- | --- |
| HID descriptor 不支持 | 关闭该 interface，记录日志，其他 interface 继续 |
| report 解码失败 | 记录前 3 次，忽略该包 |
| USB 传输错误 | 清零该 interface 的 layout 状态并通知 Translator 释放 |
| USB 事件队列满 | 丢弃事件、置 overflow、全量释放按键 |
| BLE 认证超时 | 断开 |
| 断开事件丢失 | 5 s 后强制复位 |
| NVS 数据损坏 | 使用空槽位表，不擦除 NVS |
| 切换时按键未释放 | 等全部释放后再 arm |
