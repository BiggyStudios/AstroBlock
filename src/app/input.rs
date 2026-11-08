//! Per-frame keyboard / mouse state.

use std::collections::HashSet;

use winit::{event::MouseButton, keyboard::KeyCode};

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
