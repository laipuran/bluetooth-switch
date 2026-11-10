//! Debounced button gestures shared by the physical controls.

/// A completed button gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Click,
    LongPress,
}

/// Poll-driven debouncer with click/long-press detection.
///
/// `update` is expected to be called regularly (every few milliseconds) with
/// the raw active-high state of the button. A click is emitted on release if
/// the button was held for less than `long_ms`. A long press is emitted once
/// while the button is still held.
#[derive(Clone, Debug)]
pub struct Button {
    raw: bool,
    stable: bool,
    changed_at_ms: u64,
    pressed_at_ms: u64,
    long_fired: bool,
    debounce_ms: u64,
    long_ms: u64,
}

impl Button {
    pub fn new(debounce_ms: u64, long_ms: u64) -> Self {
        Self {
            raw: false,
            stable: false,
            changed_at_ms: 0,
            pressed_at_ms: 0,
            long_fired: false,
            debounce_ms,
            long_ms,
        }
    }

    /// Feed a raw sample. Returns a gesture exactly once per press/release.
    pub fn update(&mut self, now_ms: u64, pressed: bool) -> Option<Gesture> {
        if pressed != self.raw {
            self.raw = pressed;
            self.changed_at_ms = now_ms;
        }

        let mut gesture = None;

        if self.raw != self.stable && now_ms.saturating_sub(self.changed_at_ms) >= self.debounce_ms
        {
            self.stable = self.raw;
            if self.stable {
                self.pressed_at_ms = now_ms;
                self.long_fired = false;
            } else if !self.long_fired {
                // A release can arrive after the long-press threshold without
                // an intermediate poll, so classify it here as well.
                gesture = Some(
                    if now_ms.saturating_sub(self.pressed_at_ms) >= self.long_ms {
                        Gesture::LongPress
                    } else {
                        Gesture::Click
                    },
                );
            }
        }

        if self.stable
            && !self.long_fired
            && now_ms.saturating_sub(self.pressed_at_ms) >= self.long_ms
        {
            self.long_fired = true;
            gesture = Some(Gesture::LongPress);
        }

        gesture
    }

    pub fn is_pressed(&self) -> bool {
        self.stable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_is_emitted_once_on_release() {
        let mut button = Button::new(15, 1000);
        assert_eq!(button.update(0, true), None);
        assert_eq!(button.update(10, true), None);
        assert_eq!(button.update(20, true), None);
        assert!(button.is_pressed());
        assert_eq!(button.update(30, false), None);
        assert_eq!(button.update(50, false), Some(Gesture::Click));
        assert_eq!(button.update(60, false), None);
    }

    #[test]
    fn long_press_fires_while_held_and_not_on_release() {
        let mut button = Button::new(15, 100);
        assert_eq!(button.update(0, true), None);
        assert_eq!(button.update(20, true), None);
        assert_eq!(button.update(121, true), Some(Gesture::LongPress));
        assert_eq!(button.update(150, true), None);
        assert_eq!(button.update(200, false), None);
        assert_eq!(button.update(230, false), None);
    }

    #[test]
    fn a_release_after_the_threshold_is_a_long_press() {
        let mut button = Button::new(15, 100);
        assert_eq!(button.update(0, true), None);
        assert_eq!(button.update(20, true), None);
        assert_eq!(button.update(130, false), Some(Gesture::LongPress));
        assert_eq!(button.update(150, false), None);
    }

    #[test]
    fn contact_bounce_is_ignored() {
        let mut button = Button::new(20, 1000);
        assert_eq!(button.update(0, true), None);
        assert_eq!(button.update(5, false), None);
        assert_eq!(button.update(10, true), None);
        assert_eq!(button.update(25, true), None);
        assert_eq!(button.update(35, true), None);
        assert!(button.is_pressed());
        assert_eq!(button.update(40, false), None);
        assert_eq!(button.update(70, false), Some(Gesture::Click));
    }
}
