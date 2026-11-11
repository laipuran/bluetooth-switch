//! Pure logic shared by the BT Switch firmware and host-side tests.
//!
//! Nothing in this crate depends on ESP-IDF, so it can be unit tested with a
//! regular `cargo test`. The firmware only supplies I/O around these pieces.
#![forbid(unsafe_code)]

pub mod control;
pub mod hid;
pub mod report;
pub mod router;
pub mod session;
pub mod slots;

/// HID interfaces that can be merged into one Bluetooth HID device.
///
/// A single 2.4 GHz dongle commonly exposes keyboard, mouse and consumer
/// interfaces at the same time. Eight slots are enough for a few composite
/// receivers and keep all state statically sized in the firmware.
pub const MAX_SOURCES: usize = 8;

/// Upper bound for selectable Bluetooth host slots.
pub const MAX_SLOTS: usize = 6;

pub use hid::{Decoded, Parser};
pub use report::{Reports, Sent, Translator};
pub use router::{Peer, Router};
pub use slots::{PairingWindow, Slot, SlotTable};
