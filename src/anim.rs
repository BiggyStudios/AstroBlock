//! UI motion: the overlay is immediate-mode (rebuilt every frame), so anything that eases,
//! slides or pops keeps a little state here between frames. Everything runs on real time,
//! never warped time.

use crate::{inventory::Slot, items::Item};

/// Moves `current` towards `target` exponentially: `rate` is 1/seconds (about 63% of the
/// way per 1/rate seconds), independent of the frame rate.
pub fn approach(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    target + (current - target) * (-rate * dt).exp()
}

/// Cubic ease-out of a 0..1 progress.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// What a slot holds, for change detection (wear is ignored, so using a tool doesn't pop it).
fn key(slot: &Slot) -> Option<(Item, u8)> {
    slot.map(|s| (s.item, s.count))
}

const POP_TIME: f32 = 0.24;

/// A slot that pops (briefly grows) when it gains items.
#[derive(Clone, Copy, Debug)]
pub struct Pop {
    key: Option<(Item, u8)>,
    age: f32,
}

impl Default for Pop {
    fn default() -> Self {
        Self {
            key: None,
            age: POP_TIME,
        }
    }
}

impl Pop {
    /// Starts out showing `slot`, without popping.
    pub fn new(slot: &Slot) -> Self {
        Self {
            key: key(slot),
            age: POP_TIME,
        }
    }

    /// Watches the slot this frame; returns the icon's scale (1 at rest).
    pub fn observe(&mut self, slot: &Slot, dt: f32) -> f32 {
        let now = key(slot);
        let gained = match (self.key, now) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some((a, n)), Some((b, m))) => a != b || m > n,
        };
        if gained {
            self.age = 0.0;
        }
        self.key = now;
        self.age = (self.age + dt).min(POP_TIME);
        let p = self.age / POP_TIME;
        1.0 + 0.4 * (p * std::f32::consts::PI).sin() * (1.0 - p)
    }
}

/// A vital (health, food) as shown: the line eases to the value (quickly down, gently up),
/// and a loss leaves a "trail" segment that holds for a moment, then drains after it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vital {
    pub shown: f32,
    pub trail: f32,
    /// 1 right after a loss / a gain, fading to 0.
    pub flash: f32,
    pub glow: f32,
    target: f32,
    hold: f32,
    ready: bool,
}

impl Vital {
    /// Follows `value` (0..1); returns how much was lost since the last frame.
    pub fn update(&mut self, value: f32, dt: f32) -> f32 {
        if !self.ready {
            *self = Self {
                shown: value,
                trail: value,
                target: value,
                ready: true,
                ..Self::default()
            };
            return 0.0;
        }
        let lost = (self.target - value).max(0.0);
        if lost > 1e-4 {
            self.hold = 0.45;
            self.flash = 1.0;
        } else if value > self.target + 1e-4 {
            self.glow = 1.0;
        }
        self.target = value;
        let rate = if value < self.shown { 22.0 } else { 7.0 };
        self.shown = approach(self.shown, value, rate, dt);
        if self.hold > 0.0 {
            self.hold -= dt;
        } else {
            self.trail = approach(self.trail, self.shown, 4.0, dt);
        }
        self.trail = self.trail.max(self.shown);
        self.flash = (self.flash - dt * 3.0).max(0.0);
        self.glow = (self.glow - dt * 2.0).max(0.0);
        lost
    }
}

/// Linear blend of two colours.
pub fn mix(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemStack;

    #[test]
    fn approach_is_frame_rate_independent() {
        let mut a = 0.0;
        for _ in 0..10 {
            a = approach(a, 1.0, 5.0, 0.01);
        }
        let b = approach(0.0, 1.0, 5.0, 0.1);
        assert!((a - b).abs() < 1e-5);
    }

    #[test]
    fn pops_on_gain_only() {
        let one = Some(ItemStack::new(Item::Stick, 1));
        let two = Some(ItemStack::new(Item::Stick, 2));
        let mut pop = Pop::new(&two);
        assert_eq!(pop.observe(&two, 0.016), 1.0);
        assert_eq!(pop.observe(&one, 0.016), 1.0, "losing items doesn't pop");
        assert!(pop.observe(&two, 0.05) > 1.1, "gaining does");
        for _ in 0..30 {
            pop.observe(&two, 0.016);
        }
        assert_eq!(pop.observe(&two, 0.016), 1.0, "and settles");
    }

    #[test]
    fn vital_trail_holds_then_drains() {
        let mut v = Vital::default();
        v.update(1.0, 0.016);
        assert!(v.update(0.6, 0.016) > 0.39);
        for _ in 0..10 {
            v.update(0.6, 0.016);
        }
        assert!((v.shown - 0.6).abs() < 0.05, "the line follows quickly");
        assert!(v.trail > 0.99, "the trail holds");
        for _ in 0..120 {
            v.update(0.6, 0.016);
        }
        assert!(v.trail < 0.62, "then drains");
    }
}
