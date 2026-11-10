# BT Switch（Rust 版）

把插在 ESP32-S3 USB Host 口上的 2.4 GHz 键鼠接收器，变成一个可以在多台电脑之间切换的蓝牙 HID 键鼠。

这是 `../bluetooth-switch` 中 PlatformIO/C 原型的功能等价 Rust 实现，目标板为 **Espressif ESP32-S3-USB-OTG**（ESP32-S3-MINI-1-N8，8 MB Flash，无 PSRAM）。

```text
 2.4 GHz 接收器 ──USB-A──> ESP32-S3 (USB Host)
                               │
                               │ HID report 解析 / 合并
                               ▼
                         BLE HID (HOGP)
                               │
             ┌─────────────────┼─────────────────┐
             ▼                 ▼                 ▼
          主机 1            主机 2            主机 3
```

同一时间只连接一台主机。通过板上按键或串口命令选择槽位，切换时先断开当前主机，再用白名单只向目标主机广播。

## 硬件

| 功能 | 引脚 | 说明 |
| --- | --- | --- |
| USB Host D+/D− | GPIO20 / GPIO19 | 由 `USB_SEL` 切换到 Type-A HOST 口 |
| `USB_SEL` | GPIO18 | 固件拉高，S3 原生 USB 接到 USB-A Host 口 |
| `DEV_VBUS_EN` | GPIO12 | 默认拉高：Host 口由 USB_DEV 口供电 |
| `BOOST_EN` | GPIO13 | 电池升压供电时改为拉高 |
| `LIMIT_EN` | GPIO17 | 拉高，启用 500 mA 限流 |
| 绿色 LED | GPIO15 | 已连接并准备好时常亮；广播时间歇闪烁 |
| 黄色 LED | GPIO16 | 至少一个 USB HID 接口在线 |
| OK | GPIO0 | 单击：下一个槽位；长按 3 s：打开配对窗口 |
| UP | GPIO10 | 单击：上一个槽位 |
| DW | GPIO11 | 单击：打开配对窗口 |
| MENU | GPIO14 | 单击：状态；长按 5 s：忘记当前槽位 |

> **供电注意**：默认配置下，USB-A HOST 口的 5 V 来自板上的 `USB_DEV` Type-A 公头（或电池升压）。只插 Micro-USB 调试口时，接收器可能没有供电。使用电池时把 `crates/firmware/src/board.rs` 里的 `HOST_POWER` 改成 `BatteryBoost`。
>
> **烧录注意**：固件把 `USB_SEL` 拉高后，S3 的原生 USB 已经接到 USB-A Host 口，因此**必须通过板上的 Micro-USB UART 口烧录和看日志**，不要指望 USB_DEV 口出现在电脑上。

## 目录结构

```text
crates/core/        与 ESP-IDF 无关的纯逻辑，可在电脑上 cargo test
  hid.rs            HID report descriptor 解析、report 解码、BLE report map
  report.rs         多接口状态合并、键盘/鼠标/多媒体 report、切换时的“armed”保护
  slots.rs          槽位表、NVS 二进制格式、配对窗口
  control.rs        按键消抖、单击/长按识别
crates/firmware/    ESP-IDF 固件
  usb.rs            USB Host + HID class driver（FFI）
  ble.rs            NimBLE BLE HID、白名单、认证、槽位状态机
  board.rs          ESP32-S3-USB-OTG 的引脚/供电初始化
  storage.rs        NVS 槽位持久化
  console.rs        串口命令
  main.rs           主循环
```

## 构建

### 依赖

固件使用 Espressif 维护的 Xtensa Rust 工具链（上游 nightly 在 Xtensa 上还有 LLVM 寄存器分配和 `compiler_builtins` 重定位问题）：

```sh
cargo install espup
espup install --std --targets esp32s3 \
  --toolchain-version 1.97.0.0 --skip-version-parse
cargo install ldproxy espflash
source ~/export-esp.sh          # 推荐；提供 esp-clang / LIBCLANG_PATH
```

`1.97.0.0` 是本文验证过的版本。`--skip-version-parse` 可以避开 espup 查询 GitHub API 时的限流。`crates/firmware/rust-toolchain.toml` 固定使用 `esp` 工具链；根目录的 `rust-toolchain.toml` 使用普通 nightly，仅用于 `bt-switch-core` 的电脑端测试。

### 默认：让 esp-idf-sys 自动准备 ESP-IDF

```sh
cd crates/firmware
cargo build --release
```

首次构建会下载 ESP-IDF v6.1、Xtensa GCC 和 Python 工具，耗时较长。ESP-IDF 版本在 `crates/firmware/.cargo/config.toml` 的 `ESP_IDF_VERSION` 中指定。

### 复用 PlatformIO 的 ESP-IDF（可选）

本机已经装过 PlatformIO 的 `framework-espidf 6.1.0` 时，可以避免再下载一份：

```sh
export PATH="$HOME/.platformio/penv/bin:$PATH"
export ESP_IDF_TOOLS_INSTALL_DIR=fromenv
cd crates/firmware
cargo build --release --features pio
```

### 烧录 / 监控

```sh
cd crates/firmware
cargo run --release                 # 通过 .cargo/config.toml 里的 espflash runner
```

等价的手动命令：

```sh
espflash flash --chip esp32s3 --flash-size 8mb --monitor --partition-table ../../partitions.csv \
  ../../target/xtensa-esp32s3-espidf/release/bt-switch
```

### 运行电脑端测试

```sh
cargo test -p bt-switch-core
cargo clippy -p bt-switch-core --all-targets -- -D warnings
```

`bt-switch-core` 包含 22 个测试：HID 描述符边界/模糊输入、report 解码、多接口合并、切换保护、槽位持久化、按键手势。

## 使用

固件启动后，绿色 LED 慢闪表示正在广播。首次配对：

1. 用 `n` / OK 键选中一个空槽位。选中空槽位会自动打开 120 s 配对窗口。
2. 在目标电脑的蓝牙设置里搜索 `BT Switch` 并连接。系统会走 Just Works 配对（无 PIN）。
3. 配对成功后绿色 LED 常亮，键鼠输入开始转发。

之后：

* `n` / OK：下一个槽位。
* `b` / UP：上一个槽位。
* `1`..`3`：直接选择槽位。
* `p` / DW：重新打开当前空槽位的配对窗口。
* `X` / MENU 长按 5 s：忘记当前槽位（同时会删除 BLE bond；主机侧也要移除设备）。
* `s` / MENU 单击：打印槽位、连接、USB 状态。
* `h`：帮助。

切换时如果按着 Ctrl/Alt/鼠标键，固件会等到所有键都松开才向新主机转发输入，避免“粘键”。

串口参数：115200 8N1（Micro-USB UART）。

## 实现要点

* **USB Host**：通过 `espressif/usb_host_hid` 托管组件枚举接收器。每个 HID interface 独立解析 report descriptor；支持编号 report、boot 协议设备强制切到 report 协议。
* **多接口合并**：一个接收器常常同时暴露键盘、鼠标、Consumer 三个 interface，固件按接口合并按键/按钮/多媒体键；相对位移只属于当前数据包，不会被重新播放。
* **BLE HID**：NimBLE + 自定义 HID over GATT 服务，report map 与 C 版一致（keyboard / mouse / consumer 三个 report ID）。
* **槽位**：最多 6 个（固件默认 3 个），地址写入 NVS。已配对槽位广播时使用单地址白名单；如果白名单设置失败，会退回“连接后认证校验”。
* **安全**：LE Secure Connections + Bonding，Just Works，无 MITM；HID 输入 characteristic 要求加密。
* **切换保护**：`Translator` 在切换/断线后 disarm，只有当前输入全部释放且 BLE 已就绪时才重新 arm。

更详细的状态机和线程模型见 [`docs/DESIGN.md`](docs/DESIGN.md)。

## 已知限制

* 同一时间只连接一台主机；不支持同时多主机广播。
* 只处理标准 USB HID 键鼠/Consumer；不转发厂商自定义 report、RGB、震动等。
* HID 输出 report（键盘 LED）已经建立 characteristic，但尚未回传给 USB 接收器。
* 绝对坐标设备（数位板、触摸屏）会被忽略，避免被误当成鼠标相对位移。
* 蓝牙键盘 report 使用 boot 格式，最多 6 键同时按下；超过 6 键发送 HID ErrorRollOver。
* USB Hub 未启用（ESP-IDF `CONFIG_USB_HOST_HUBS_SUPPORTED` 默认关闭）。
* 没有实现旋钮/编码器；`control::Button` 和 `main.rs` 的按键映射很容易替换成编码器或 OLED 菜单。

## 与 C 版本的关系

PlatformIO/C 版本仍在原工作区的 `main` 分支；本仓库是一个独立（orphan）的 `rust` 分支。两个实现共享同一套 HID report map 和槽位行为，方便对照。

## 许可证

MIT OR Apache-2.0
