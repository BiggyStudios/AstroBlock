//! Per-frame keyboard / mouse state.

use std::collections::HashSet;

use winit::{event::MouseButton, keyboard::KeyCode};

/// How long smoothed mouse movement takes to catch up (s): long enough to even out the uneven
/// amounts of movement each frame gets, short enough that the view doesn't feel behind the hand.
const SMOOTHING_TIME: f64 = 0.03;

/// Spreads mouse movement over a few frames. Exponential, so every frame rate feels the same.
/// Nothing is lost: a sweep of the mouse turns the view just as far as without it.
#[derive(Default)]
pub struct MouseSmoother {
    left: (f64, f64),
}

impl MouseSmoother {
    /// Takes this frame's movement and gives how much of it to apply now.
    pub fn step(&mut self, moved: (f64, f64), dt: f64, on: bool) -> (f64, f64) {
        if !on {
            self.clear();
            return moved;
        }
        let part = 1.0 - (-dt / SMOOTHING_TIME).exp();
        let now = (
            (self.left.0 + moved.0) * part,
            (self.left.1 + moved.1) * part,
        );
        self.left.0 += moved.0 - now.0;
        self.left.1 += moved.1 - now.1;
        now
    }

    pub fn clear(&mut self) {
        self.left = (0.0, 0.0);
    }
}

#[derive(Default)]
pub struct Input {
    held: HashSet<KeyCode>,
    pressed: HashSet<KeyCode>,
    buttons_held: HashSet<MouseButton>,
    buttons_pressed: HashSet<MouseButton>,
    /// Raw mouse movement accumulated this frame.
    pub mouse_delta: (f64, f64),
    /// Mouse cursor position in window pixels (when it isn't captured).
    pub cursor: (f32, f32),
    /// Scroll wheel notches accumulated this frame.
    pub scroll: f32,
}

impl Input {
    pub fn key_down(&mut self, key: KeyCode) {
        if self.held.insert(key) {
            self.pressed.insert(key);
        }
    }

    pub fn key_up(&mut self, key: KeyCode) {
        self.held.remove(&key);
    }

    pub fn button_down(&mut self, button: MouseButton) {
        if self.buttons_held.insert(button) {
            self.buttons_pressed.insert(button);
        }
    }

    pub fn button_up(&mut self, button: MouseButton) {
        self.buttons_held.remove(&button);
    }

    /// Key is currently held down.
    pub fn held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    /// Key went down this frame.
    pub fn pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    pub fn button_held(&self, button: MouseButton) -> bool {
        self.buttons_held.contains(&button)
    }

    pub fn button_held_left(&self) -> bool {
        self.button_held(MouseButton::Left)
    }

    pub fn button_pressed(&self, button: MouseButton) -> bool {
        self.buttons_pressed.contains(&button)
    }

    /// Releases all keys and buttons, e.g. when the window loses focus (we'd miss the
    /// release events).
    pub fn clear(&mut self) {
        self.held.clear();
        self.buttons_held.clear();
        self.end_frame();
    }

    pub fn end_frame(&mut self) {
        self.pressed.clear();
        self.buttons_pressed.clear();
        self.mouse_delta = (0.0, 0.0);
        self.scroll = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One flick of the mouse, then `frames` frames of `dt`: how far the view has turned.
    fn after_a_flick(dt: f64, frames: usize) -> f64 {
        let mut smoother = MouseSmoother::default();
        let mut turned = smoother.step((100.0, 0.0), dt, true).0;
        for _ in 1..frames {
            turned += smoother.step((0.0, 0.0), dt, true).0;
        }
        turned
    }

    #[test]
    fn smoothing_loses_no_movement() {
        for fps in [30.0, 60.0, 144.0, 360.0] {
            let frames = (fps * 0.5) as usize;
            assert!((after_a_flick(1.0 / fps, frames) - 100.0).abs() < 1e-3);
        }
    }

    #[test]
    fn smoothing_feels_the_same_at_any_frame_rate() {
        let slow = after_a_flick(1.0 / 60.0, 3);
        let fast = after_a_flick(1.0 / 240.0, 12);
        assert!((slow - fast).abs() < 1e-9, "{slow} against {fast}");
        // It is spread out, and has mostly arrived within a tenth of a second.
        assert!(after_a_flick(1.0 / 120.0, 1) < 40.0);
        assert!(after_a_flick(1.0 / 120.0, 12) > 95.0);
    }

    #[test]
    fn smoothing_off_changes_nothing() {
        let mut smoother = MouseSmoother::default();
        smoother.step((50.0, 7.0), 0.01, true);
        assert_eq!(smoother.step((3.0, -2.0), 0.01, false), (3.0, -2.0));
        // And what was still on its way doesn't come back later.
        assert_eq!(smoother.step((0.0, 0.0), 0.01, true), (0.0, 0.0));
    }
}
