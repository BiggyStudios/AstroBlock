//! Immediate-mode 2D overlay: rectangles, text, block icons and item sprites in screen
//! pixels, rebuilt every frame and drawn on top of everything (see `render::UiVertex`).

pub mod anim;
pub mod console;
pub mod hud;
pub mod menu;
pub mod screen;

use crate::{
    render::{UiVertex, font},
    survival::items::{Icon, ItemStack},
    world::{
        block::{Block, Tint},
        textures::Tex,
    },
};

/// `UiVertex::layer` for font/solid quads. Otherwise bits 0-7 are a texture array layer,
/// bits 8-15 a brightness (0 = full) and bit 31 marks sprites (alpha = transparency rather
/// than the biome tint mask). Must match `shaders/ui.frag`.
pub const FONT_LAYER: u32 = u32::MAX;
const SPRITE: u32 = 1 << 31;

pub struct Ui {
    pub vertices: Vec<UiVertex>,
    pub width: f32,
    pub height: f32,
    /// Pixel size of one font pixel (and one "GUI pixel" of the inventory screens).
    pub scale: f32,
}

impl Ui {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            vertices: Vec::new(),
            width,
            height,
            scale: (height / 360.0).round().max(1.0),
        }
    }

    /// Any quad: 4 corners (in order around it) with their uvs.
    pub fn quad_points(
        &mut self,
        points: [[f32; 2]; 4],
        uvs: [[f32; 2]; 4],
        color: [u8; 4],
        layer: u32,
    ) {
        for (pos, uv) in points.into_iter().zip(uvs) {
            self.vertices.push(UiVertex {
                pos,
                uv,
                color,
                layer,
            });
        }
    }

    fn quad(
        &mut self,
        (x, y, w, h): (f32, f32, f32, f32),
        (a, b): ([f32; 2], [f32; 2]),
        color: [u8; 4],
        layer: u32,
    ) {
        self.quad_points(
            [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            [a, [b[0], a[1]], b, [a[0], b[1]]],
            color,
            layer,
        );
    }

    /// A filled rectangle (sRGB colour with alpha).
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        // Sample the middle of the solid cell so filtering never reaches its edge.
        let (a, b) = font::glyph_uv(font::SOLID);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        self.quad((x, y, w, h), (mid, mid), color, FONT_LAYER);
    }

    /// A rectangle with its own colour at each corner (top-left, top-right, bottom-right,
    /// bottom-left), blended across it.
    pub fn gradient(&mut self, x: f32, y: f32, w: f32, h: f32, colors: [[u8; 4]; 4]) {
        let (a, b) = font::glyph_uv(font::SOLID);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let points = [[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        for (pos, color) in points.into_iter().zip(colors) {
            self.vertices.push(UiVertex {
                pos,
                uv: mid,
                color,
                layer: FONT_LAYER,
            });
        }
    }

    /// Any quad with its own colour at each corner (solid, no texture).
    pub fn poly(&mut self, points: [[f32; 2]; 4], colors: [[u8; 4]; 4]) {
        let (a, b) = font::glyph_uv(font::SOLID);
        let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        for (pos, color) in points.into_iter().zip(colors) {
            self.vertices.push(UiVertex {
                pos,
                uv: mid,
                color,
                layer: FONT_LAYER,
            });
        }
    }

    /// A straight line `width` pixels wide.
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], width: f32, color: [u8; 4]) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            return;
        }
        let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
        self.poly(
            [
                [a[0] + nx, a[1] + ny],
                [b[0] + nx, b[1] + ny],
                [b[0] - nx, b[1] - ny],
                [a[0] - nx, a[1] - ny],
            ],
            [color; 4],
        );
    }

    /// A circle outline.
    pub fn circle(&mut self, center: [f32; 2], radius: f32, width: f32, color: [u8; 4]) {
        let n = ((radius * 0.8) as usize).clamp(10, 64);
        let point = |i: usize| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [center[0] + radius * a.cos(), center[1] + radius * a.sin()]
        };
        for i in 0..n {
            self.line(point(i), point(i + 1), width, color);
        }
    }

    /// Fades (multiplies the alpha) and moves everything drawn since vertex `start`: how a
    /// group of quads is animated after being drawn as usual. Moves snap to whole pixels so
    /// the pixel font stays crisp.
    pub fn fade_from(&mut self, start: usize, alpha: f32, dx: f32, dy: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        let (dx, dy) = (dx.round(), dy.round());
        for v in &mut self.vertices[start..] {
            v.color[3] = (v.color[3] as f32 * alpha).round() as u8;
            v.pos[0] += dx;
            v.pos[1] += dy;
        }
    }

    /// A rectangle outline `t` pixels thick.
    pub fn frame(&mut self, x: f32, y: f32, w: f32, h: f32, t: f32, color: [u8; 4]) {
        self.rect(x, y, w, t, color);
        self.rect(x, y + h - t, w, t, color);
        self.rect(x, y + t, t, h - 2.0 * t, color);
        self.rect(x + w - t, y + t, t, h - 2.0 * t, color);
    }

    /// Text with its top-left corner at (x, y), with a soft dark outline so it reads over
    /// any background; returns the width drawn.
    pub fn text(&mut self, x: f32, y: f32, text: &str, color: [u8; 4]) -> f32 {
        self.glyphs(x, y, text, color, true)
    }

    /// Text without the outline.
    pub fn text_plain(&mut self, x: f32, y: f32, text: &str, color: [u8; 4]) -> f32 {
        self.glyphs(x, y, text, color, false)
    }

    fn glyphs(&mut self, x: f32, y: f32, text: &str, color: [u8; 4], outline: bool) -> f32 {
        let (cw, ch) = self.char_size();
        let s = self.scale;
        let mut cx = x;
        for c in text.chars() {
            if c != ' ' {
                let uv = font::glyph_uv(c);
                if outline {
                    let halo = [0, 0, 0, color[3] / 3];
                    for (dx, dy) in [(-s, 0.0), (s, 0.0), (0.0, -s), (0.0, s)] {
                        self.quad((cx + dx, y + dy, cw, ch), uv, halo, FONT_LAYER);
                    }
                }
                self.quad((cx, y, cw, ch), uv, color, FONT_LAYER);
            }
            cx += cw;
        }
        cx - x
    }

    pub fn text_width(&self, text: &str) -> f32 {
        text.chars().count() as f32 * self.char_size().0
    }

    pub fn char_size(&self) -> (f32, f32) {
        (
            font::CELL.0 as f32 * self.scale,
            font::CELL.1 as f32 * self.scale,
        )
    }

    /// A sprite (alpha = transparency), multiplied by `color`. `uv` selects part of it.
    pub fn sprite_part(
        &mut self,
        (x, y, w, h): (f32, f32, f32, f32),
        tex: Tex,
        uv: ([f32; 2], [f32; 2]),
        color: [u8; 4],
    ) {
        self.quad((x, y, w, h), uv, color, tex as u32 | SPRITE);
    }

    pub fn sprite(&mut self, x: f32, y: f32, size: f32, tex: Tex, color: [u8; 4]) {
        self.sprite_part((x, y, size, size), tex, ([0.0; 2], [1.0; 2]), color);
    }

    /// A block as a flat tile of its side texture, with a lighter top edge and a darker
    /// bottom edge (a hint of depth, no 3D).
    pub fn block_icon(&mut self, x: f32, y: f32, size: f32, block: Block) {
        let [top, side, _] = block.icon_faces();
        // Grass reads best from above; everything else by its side.
        let tex = if block == Block::Grass { top } else { side };
        let tint = match block.tint() {
            Tint::Grass => [124, 189, 81, 255],
            Tint::Foliage => [90, 160, 60, 255],
            Tint::Spruce => [90, 125, 90, 255],
            Tint::Water => [60, 110, 220, 255],
            Tint::None => [255, 255, 255, 255],
        };
        // See-through blocks (glass) are drawn as sprites: alpha is transparency.
        let sprite = if block.is_cutout() { SPRITE } else { 0 };
        self.quad(
            (x, y, size, size),
            ([0.0; 2], [1.0; 2]),
            tint,
            tex as u32 | sprite,
        );
        let edge = (self.scale).max(1.0);
        self.rect(x, y, size, edge, [255, 255, 255, 40]);
        self.rect(x, y + size - edge, size, edge, [0, 0, 0, 70]);
    }

    /// An item stack: its icon, a small count and a thin durability line for worn tools.
    pub fn item(&mut self, x: f32, y: f32, size: f32, stack: &ItemStack) {
        match stack.item.icon() {
            Icon::Block(block) => {
                let inset = size * 0.12;
                self.block_icon(x + inset, y + inset, size - 2.0 * inset, block);
            }
            Icon::Sprite(tex) => self.sprite(x, y, size, tex, [255; 4]),
        }
        let s = self.scale;
        if stack.count > 1 {
            let text = stack.count.to_string();
            let (w, h) = (self.text_width(&text), self.char_size().1);
            self.text(
                x + size - w + s,
                y + size - h + 2.0 * s,
                &text,
                [235, 240, 245, 255],
            );
        }
        if let Some(left) = stack.durability()
            && stack.wear > 0
        {
            let color = if left > 0.25 {
                [210, 230, 240, 230]
            } else {
                [240, 90, 70, 255]
            };
            self.rect(x, y + size, size * left, s, color);
        }
    }
}
