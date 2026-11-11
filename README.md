# BT Switch（Rust 版）

把插在 ESP32-S3 USB Host 口上的 2.4 GHz 键鼠接收器，变成一个可以在多台电脑之间切换的蓝牙 HID 键鼠。

这是 `../bluetooth-switch` 中 PlatformIO/C 原型的 Rust 移植项目。当前测试板为第三方 ESP32-S3 开发板，串口识别到 16 MB Flash；固件已接入多连接路由，实际验证范围见下文。

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

默认配置最多 3 条连接，仅选中的槽位接收输入。切槽不再主动断连；旧、新目标发送全释放，失败会重试。离线目标暂停输入，不回退到其他主机；白名单覆盖全部已配对槽位。

**上板状态（2026-11-12）**：按用户授权整片擦除、仅重刷 Rust 后，已验证两台 Linux 同时加密在线、双端订阅、40 次切换的旧新目标释放与通知隔离、离线第三槽位、只忘记单槽位，以及连续 10 次复位后恢复双机加密。
本轮修复了广播期间删除 bond 失败及底层 SMP 过程池容量不足的问题。测试只使用全零报告；**USB→BLE 完整输入链路、第三台在线、其他系统及 RPA 仍未验证**。详见 [上板测试记录](docs/BOARD_TEST.md)。

## 硬件

默认使用通用板配置：**不驱动任何未确认的 LED、按键、USB 路由或供电 GPIO**，管理操作使用串口。
原生 USB 使用 GPIO19=D−、GPIO20=D+；使用 USB Host 前须确认对应接口接线及 5 V VBUS，不能根据双 Type-C 外观判断。
本轮实读 Flash 为 16 MB，配置和烧录参数已据此设置；更换硬件须重新确认容量。

以下引脚表**仅适用于乐鑫官方 ESP32-S3-USB-OTG 板**，需显式使用 `--features official-otg`。
官方板还需将 `sdkconfig.defaults` 和烧录命令中的 Flash 容量改为实际 8 MB，不能直接使用当前测试板参数。

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

> **官方板供电注意**：启用 `official-otg` 后，USB-A HOST 口的 5 V 来自板上的 `USB_DEV` Type-A 公头（或电池升压）。只插 Micro-USB 调试口时，接收器可能没有供电。使用电池时把 `crates/firmware/src/board/official_otg.rs` 里的 `HOST_POWER` 改成 `BatteryBoost`。
>
> **烧录注意**：请使用板上的 USB-UART 接口。官方板启用 `USB_SEL` 后，S3 原生 USB 已接到 Host 口，不能再用它烧录或监控。

## 目录结构

```text
crates/core/        与 ESP-IDF 无关的纯逻辑，可在电脑上 cargo test
  hid.rs            HID report descriptor 解析、report 解码、BLE report map
  report.rs         多接口状态合并、键盘/鼠标/多媒体 report、切换时的“armed”保护
  router/           已接入固件的多连接路由、定向投递与释放重试
  session.rs        逐连接协议、订阅、挂起、LED 与读取快照
  slots.rs          槽位表、NVS 二进制格式、配对窗口
  control.rs        按键消抖、单击/长按识别
crates/firmware/    ESP-IDF 固件
  usb.rs            USB Host + HID class driver（FFI）
  ble/              BLE 应用层与 NimBLE 适配层，职责划分见 docs/DESIGN.md
  board/            默认通用安全配置；官方 OTG 引脚配置需显式启用
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
espflash flash --chip esp32s3 --flash-size 16mb --monitor --partition-table ../../partitions.csv \
  ../../target/xtensa-esp32s3-espidf/release/bt-switch
```

### 运行电脑端测试

```sh
cargo test -p bt-switch-core
cargo clippy -p bt-switch-core --all-targets -- -D warnings
```

`bt-switch-core` 包含 33 个测试：HID 描述符边界/模糊输入、report 解码、多接口合并、切换保护、多连接路由、逐连接属性、槽位持久化、按键手势。

## 使用

通用板通过串口 `s` 查看状态，不操作板载 LED 或按键。下述 LED/按键提示仅适用于官方板配置。首次配对：

1. 用 `p` 为空槽位打开 120 s 配对窗口，不改变输入目标或断开已有主机；选择空槽位也会打开窗口。
2. 在目标电脑的蓝牙设置里搜索 `BT Switch` 并连接。系统会走 Just Works 配对（无 PIN）。
3. 用 `1`..`3` 选择输入目标。目标必须已加密、有真实 HID 通知订阅，且全释放已成功排队，才能接收输入。

之后：

* `n` / OK：下一个槽位。
* `b` / UP：上一个槽位。
* `1`..`3`：直接选择槽位。
* `t`：向当前主机发送全零 HID 探针，不产生按键、鼠标移动或滚轮。
* `p` / DW：为当前空槽位或下一个空槽位打开配对窗口，不改变输入目标。
* `X` / MENU 长按 5 s：仅断开并忘记当前槽位，其他连接保留；主机侧也要移除设备。
* `s` / MENU 单击：打印槽位、连接、USB 状态。
* `h`：帮助。

切换时如果按着 Ctrl/Alt/鼠标键，固件会等到所有键都松开才向新主机转发输入，避免“粘键”。

串口参数：115200 8N1，使用实际开发板的 USB-UART 接口。

## 实现要点

* **USB Host**：通过 `espressif/usb_host_hid` 托管组件枚举接收器。每个 HID interface 独立解析 report descriptor；支持编号 report、boot 协议设备强制切到 report 协议。
* **多接口合并**：一个接收器常常同时暴露键盘、鼠标、Consumer 三个 interface，固件按接口合并按键/按钮/多媒体键；相对位移只属于当前数据包，不会被重新播放。
* **BLE HID**：NimBLE + 自定义 HID over GATT 服务，report map 与 C 版一致（keyboard / mouse / consumer 三个 report ID）。
* **槽位与连接**：核心最多 6 个槽位，固件和 NimBLE 默认配置 3 条连接；增加槽位数需同步增加控制器活动与 CCCD 容量。所有已配对主机按身份回到原槽位，与当前输入目标无关。
* **逐连接属性**：Report/Boot 协议、通知订阅、挂起、LED 与输入报告读取值彼此独立。通知只调用 `notify_with(data, connection)`，不广播给其他订阅者。
* **安全**：LE Secure Connections + Bonding，Just Works，无 MITM；动态 HID 读写及输入 CCCD 写入要求加密。新连接的应用侧安全请求串行发起。
* **SMP 容量**：仅增加 `MAX_CONNECTIONS` 不够。ESP-IDF v6.1 默认 SMP 过程池仍为 1；`ble_capacity` 本地构建配置将真实 NimBLE C 编译容量同步到连接数，不修改 SDK 或第三方 crate 源码。
* **切换保护**：`Translator` 在目标或相关连接状态变化后 disarm，需先物理释放；路由器还负责旧、新目标释放，旧主机的发送失败不阻塞新主机。

更详细的状态机和线程模型见 [`docs/DESIGN.md`](docs/DESIGN.md)。

## 已知限制

* 已完成两台 Linux 的 BLE 实机回归；第三台、RPA、其他系统与真实 USB 输入仍待验证，不能沿用 C 固件的验证结论。
* 全零释放的主机侧观测延迟受串口、连接调度与 BlueZ 影响；本轮完整三报告释放约 67–244 ms，不是 USB 输入延迟或硬实时保证。
* Boot 模式仅键盘与 3 字节鼠标，没有 Consumer/滚轮；Boot 与逐连接写属性目前只有核心测试，BlueZ 默认只读导出 HID 属性使本机 D-Bus 写属性测试受阻，未为此修改系统蓝牙配置。
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
