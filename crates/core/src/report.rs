//! Conversion from decoded HID state to the Bluetooth HID report map.
//!
//! The translation deliberately mirrors the C prototype's "armed" behaviour:
//! after a host switch the firmware waits until every key, mouse button and
//! consumer usage is released before forwarding input. That prevents a held
//! Ctrl/Alt or mouse button from leaking into the newly selected host.

use crate::MAX_SOURCES;
use crate::hid::{Decoded, KEY_BYTES, kind};

pub const KEYBOARD_REPORT_LEN: usize = 8;
pub const MOUSE_REPORT_LEN: usize = 7;
pub const CONSUMER_REPORT_LEN: usize = 2;

pub const ZERO_KEYBOARD: [u8; KEYBOARD_REPORT_LEN] = [0; KEYBOARD_REPORT_LEN];
pub const ZERO_MOUSE: [u8; MOUSE_REPORT_LEN] = [0; MOUSE_REPORT_LEN];
pub const ZERO_CONSUMER: [u8; CONSUMER_REPORT_LEN] = [0; CONSUMER_REPORT_LEN];

/// Reports that the firmware should try to send this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reports {
    pub keyboard: Option<[u8; KEYBOARD_REPORT_LEN]>,
    pub mouse: Option<[u8; MOUSE_REPORT_LEN]>,
    pub consumer: Option<[u8; CONSUMER_REPORT_LEN]>,
}

impl Reports {
    pub fn is_empty(&self) -> bool {
        self.keyboard.is_none() && self.mouse.is_none() && self.consumer.is_none()
    }
}

/// Which reports the BLE layer actually managed to notify.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sent {
    pub keyboard: bool,
    pub mouse: bool,
    pub consumer: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Motion {
    x: i32,
    y: i32,
    wheel: i32,
    pan: i32,
}

impl Motion {
    fn any(&self) -> bool {
        self.x != 0 || self.y != 0 || self.wheel != 0 || self.pan != 0
    }

    fn add(&mut self, decoded: &Decoded) {
        self.x = self.x.saturating_add(decoded.x);
        self.y = self.y.saturating_add(decoded.y);
        self.wheel = self.wheel.saturating_add(decoded.wheel);
        self.pan = self.pan.saturating_add(decoded.pan);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Merges up to [`MAX_SOURCES`] HID interfaces into one HID report stream.
pub struct Translator {
    sources: [Decoded; MAX_SOURCES],
    present: [bool; MAX_SOURCES],
    ready: bool,
    armed: bool,
    last_keyboard: [u8; KEYBOARD_REPORT_LEN],
    last_buttons: u8,
    last_consumer: u16,
    motion: Motion,
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator {
    pub fn new() -> Self {
        Self {
            sources: [Decoded::default(); MAX_SOURCES],
            present: [false; MAX_SOURCES],
            ready: false,
            armed: false,
            last_keyboard: ZERO_KEYBOARD,
            last_buttons: 0,
            last_consumer: 0,
            motion: Motion::default(),
        }
    }

    /// Update the state of one interface.
    ///
    /// Relative motion is only accumulated while input is armed. Motion that
    /// arrives while the firmware is waiting for a neutral state is discarded,
    /// just like the C implementation.
    pub fn update(&mut self, source: usize, decoded: &Decoded) {
        if source >= MAX_SOURCES {
            return;
        }
        self.present[source] = true;
        self.sources[source] = *decoded;
        if self.ready && self.armed && decoded.kinds & kind::MOUSE != 0 {
            self.motion.add(decoded);
        }
    }

    /// Remove an interface and release everything it was holding.
    pub fn detach(&mut self, source: usize) {
        if source >= MAX_SOURCES {
            return;
        }
        self.sources[source] = Decoded::default();
        self.present[source] = false;
    }

    /// Drop all state (used for USB queue overflows and hard resets).
    pub fn reset(&mut self) {
        self.sources = [Decoded::default(); MAX_SOURCES];
        self.present = [false; MAX_SOURCES];
        self.disarm();
    }

    /// Called whenever the BLE link becomes ready or stops being ready.
    pub fn set_ready(&mut self, ready: bool) {
        if !ready {
            self.disarm();
        }
        self.ready = ready;
    }

    /// Forget what the selected host has been told and stop forwarding input.
    pub fn disarm(&mut self) {
        self.armed = false;
        self.last_keyboard = ZERO_KEYBOARD;
        self.last_buttons = 0;
        self.last_consumer = 0;
        self.motion.clear();
    }

    /// Produce the reports that need to be sent.
    pub fn flush(&self) -> Reports {
        if !self.ready {
            return Reports::default();
        }

        let (keys, buttons, consumer) = self.aggregate();

        if !self.armed {
            if !neutral(&keys, buttons, consumer) {
                return Reports::default();
            }
            // Release every report once before accepting input. This also
            // covers a host that reconnects while the previous link was torn
            // down mid-keypress.
            return Reports {
                keyboard: Some(ZERO_KEYBOARD),
                mouse: Some(ZERO_MOUSE),
                consumer: Some(ZERO_CONSUMER),
            };
        }

        let mut reports = Reports::default();
        let keyboard = keyboard_report(&keys);
        if keyboard != self.last_keyboard {
            reports.keyboard = Some(keyboard);
        }
        if consumer != self.last_consumer {
            reports.consumer = Some(consumer.to_le_bytes());
        }
        if self.motion.any() || buttons != self.last_buttons {
            reports.mouse = Some(mouse_report(buttons, &self.motion));
        }
        reports
    }

    /// Remember which reports were sent so unsent state is retried next tick.
    pub fn confirm(&mut self, reports: &Reports, sent: Sent) {
        if !self.armed {
            // Some hosts only subscribe to a subset of HID reports. Arming on
            // the first successful zero report avoids blocking all input just
            // because, for example, the Consumer report is unused. A host that
            // has not subscribed to anything retries on the next tick.
            if sent.keyboard || sent.mouse || sent.consumer {
                self.armed = true;
                self.last_keyboard = ZERO_KEYBOARD;
                self.last_buttons = 0;
                self.last_consumer = 0;
                self.motion.clear();
            }
            return;
        }

        if sent.keyboard
            && let Some(report) = reports.keyboard
        {
            self.last_keyboard = report;
        }
        if sent.consumer
            && let Some(report) = reports.consumer
        {
            self.last_consumer = u16::from_le_bytes(report);
        }
        if sent.mouse
            && let Some(report) = reports.mouse
        {
            self.last_buttons = report[0];
            self.motion.clear();
        }
    }

    /// True once the first release reports have been acknowledged.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    fn aggregate(&self) -> ([u8; KEY_BYTES], u8, u16) {
        let mut keys = [0u8; KEY_BYTES];
        let mut buttons = 0u8;
        let mut consumer = 0u16;
        for (index, source) in self.sources.iter().enumerate() {
            if !self.present[index] {
                continue;
            }
            for (dst, src) in keys.iter_mut().zip(source.keys.iter()) {
                *dst |= *src;
            }
            buttons |= source.buttons;
            if consumer == 0 {
                consumer = source.consumer;
            }
        }
        (keys, buttons, consumer)
    }
}

fn neutral(keys: &[u8; KEY_BYTES], buttons: u8, consumer: u16) -> bool {
    buttons == 0 && consumer == 0 && keys.iter().all(|byte| *byte == 0)
}

/// Build the boot-format keyboard report.
pub fn keyboard_report(keys: &[u8; KEY_BYTES]) -> [u8; KEYBOARD_REPORT_LEN] {
    let mut report = [0u8; KEYBOARD_REPORT_LEN];
    // Bits 0xE0..=0xE7 live in keys[28] and map exactly to the modifier byte.
    report[0] = keys[0xe0 / 8];

    let mut count = 0usize;
    for usage in 4u16..0xe0 {
        if keys[usage as usize / 8] & (1 << (usage % 8)) != 0 {
            if count < 6 {
                report[2 + count] = usage as u8;
            }
            count += 1;
        }
    }
    // Usages 0x01..=0x03 are error/rollover codes. If one is set, or if more
    // than six regular keys are down, report HID ErrorRollOver in all slots.
    if (1..=3u16).any(|usage| keys[usage as usize / 8] & (1 << (usage % 8)) != 0) {
        count = 7;
    }
    if count > 6 {
        report[2..].fill(0x01);
    }
    report
}

/// Build the mouse report. Deltas are bounded before the wire conversion.
fn mouse_report(buttons: u8, motion: &Motion) -> [u8; MOUSE_REPORT_LEN] {
    let dx = motion.x.clamp(-32767, 32767);
    let dy = motion.y.clamp(-32767, 32767);
    let wheel = motion.wheel.clamp(-127, 127) as i8 as u8;
    let pan = motion.pan.clamp(-127, 127) as i8 as u8;
    [
        buttons,
        dx as u8,
        (dx >> 8) as u8,
        dy as u8,
        (dy >> 8) as u8,
        wheel,
        pan,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_with(usage: u16) -> [u8; KEY_BYTES] {
        let mut keys = [0u8; KEY_BYTES];
        keys[usage as usize / 8] |= 1 << (usage % 8);
        keys
    }

    fn arm(translator: &mut Translator) {
        translator.set_ready(true);
        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some(ZERO_KEYBOARD));
        translator.confirm(
            &reports,
            Sent {
                keyboard: true,
                mouse: true,
                consumer: true,
            },
        );
        assert!(translator.is_armed());
    }

    #[test]
    fn arming_waits_for_a_neutral_state() {
        let mut translator = Translator::new();
        let held = Decoded {
            keys: keys_with(4),
            ..Default::default()
        };
        translator.update(0, &held);

        translator.set_ready(true);
        assert!(translator.flush().is_empty());

        translator.update(0, &Decoded::default());
        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some(ZERO_KEYBOARD));
        assert_eq!(reports.mouse, Some(ZERO_MOUSE));
        assert_eq!(reports.consumer, Some(ZERO_CONSUMER));
        translator.confirm(
            &reports,
            Sent {
                keyboard: true,
                mouse: true,
                consumer: true,
            },
        );

        translator.update(0, &held);
        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some([0, 0, 4, 0, 0, 0, 0, 0]));
        assert_eq!(reports.mouse, None);
    }

    #[test]
    fn disarm_never_replays_held_keys_on_the_next_host() {
        let mut translator = Translator::new();
        arm(&mut translator);

        let held = Decoded {
            keys: keys_with(4),
            ..Default::default()
        };
        translator.update(0, &held);
        let reports = translator.flush();
        assert!(reports.keyboard.is_some());
        translator.confirm(
            &reports,
            Sent {
                keyboard: true,
                mouse: false,
                consumer: false,
            },
        );

        translator.set_ready(false);
        translator.set_ready(true);
        assert!(
            translator.flush().is_empty(),
            "held key must not leak across a switch"
        );

        translator.update(0, &Decoded::default());
        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some(ZERO_KEYBOARD));
        translator.confirm(
            &reports,
            Sent {
                keyboard: true,
                mouse: true,
                consumer: true,
            },
        );
        assert!(translator.is_armed());
    }

    #[test]
    fn mouse_motion_is_retried_until_sent() {
        let mut translator = Translator::new();
        arm(&mut translator);

        let motion = Decoded {
            kinds: kind::MOUSE,
            x: 5,
            y: -3,
            ..Default::default()
        };
        translator.update(0, &motion);

        let reports = translator.flush();
        assert_eq!(reports.mouse, Some([0, 5, 0, 0xfd, 0xff, 0, 0]));
        translator.confirm(
            &reports,
            Sent {
                keyboard: false,
                mouse: false,
                consumer: false,
            },
        );
        // The failed report is retried, not lost.
        assert_eq!(translator.flush().mouse, Some([0, 5, 0, 0xfd, 0xff, 0, 0]));

        translator.confirm(
            &reports,
            Sent {
                keyboard: false,
                mouse: true,
                consumer: false,
            },
        );
        assert_eq!(translator.flush().mouse, None);
    }

    #[test]
    fn modifiers_and_rollover_match_the_boot_report_format() {
        let mut keys = [0u8; KEY_BYTES];
        keys[28] |= 1 << 5; // right shift (0xE5) lives in the modifier byte
        keys[0] |= 1 << 4; // a
        keys[0] |= 1 << 5; // b
        let report = keyboard_report(&keys);
        assert_eq!(report[0], 0x20);
        assert_eq!(&report[2..4], &[4, 5]);

        let mut many = [0u8; KEY_BYTES];
        for usage in 4..12u16 {
            many[usage as usize / 8] |= 1 << (usage % 8);
        }
        assert_eq!(&keyboard_report(&many)[2..], &[1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn multiple_sources_are_merged_and_detach_releases() {
        let mut translator = Translator::new();
        arm(&mut translator);

        let keyboard = Decoded {
            kinds: kind::KEYBOARD,
            keys: keys_with(4),
            ..Default::default()
        };
        translator.update(0, &keyboard);

        let mouse = Decoded {
            kinds: kind::MOUSE,
            buttons: 0x01,
            x: 2,
            ..Default::default()
        };
        translator.update(1, &mouse);

        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some([0, 0, 4, 0, 0, 0, 0, 0]));
        assert_eq!(reports.mouse, Some([1, 2, 0, 0, 0, 0, 0]));
        translator.confirm(
            &reports,
            Sent {
                keyboard: true,
                mouse: true,
                consumer: false,
            },
        );

        translator.detach(0);
        translator.detach(1);
        let reports = translator.flush();
        assert_eq!(reports.keyboard, Some(ZERO_KEYBOARD));
        assert_eq!(reports.mouse, Some(ZERO_MOUSE));
    }

    #[test]
    fn a_single_subscribed_report_is_enough_to_arm() {
        let mut translator = Translator::new();
        translator.set_ready(true);
        let reports = translator.flush();

        // Nothing subscribed yet: keep waiting.
        translator.confirm(&reports, Sent::default());
        assert!(!translator.is_armed());

        // A mouse-only host does not subscribe to the Consumer report.
        translator.confirm(
            &reports,
            Sent {
                keyboard: false,
                mouse: true,
                consumer: false,
            },
        );
        assert!(translator.is_armed());
    }

    #[test]
    fn consumer_usage_is_little_endian() {
        let mut translator = Translator::new();
        arm(&mut translator);

        let consumer = Decoded {
            kinds: kind::CONSUMER,
            consumer: 0x0238,
            ..Default::default()
        };
        translator.update(0, &consumer);
        assert_eq!(translator.flush().consumer, Some([0x38, 0x02]));

        translator.confirm(
            &Reports {
                consumer: Some([0x38, 0x02]),
                ..Default::default()
            },
            Sent {
                consumer: true,
                ..Default::default()
            },
        );
        translator.update(0, &Decoded::default());
        assert_eq!(translator.flush().consumer, Some([0, 0]));
    }
}
