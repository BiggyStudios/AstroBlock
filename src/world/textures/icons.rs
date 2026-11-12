//! Pictures only the interface shows: the icons of the machines too big or too odd in shape
//! for a flat tile of one of their faces (`items::Icon::Picture`). Each is the machine's
//! silhouette, drawn like the item sprites: flat shapes, lit from the top left, outlined.
//!
//! They are kept after `Tex` in the texture array (`IconTex::layer`). Meshes in the world
//! can't reach them (`BlockVertex` has 8 bits for the layer, and `Tex` fills most of them):
//! a dropped or carried machine shows its model instead.

use super::{Img, Rgb, Tex, scale, sprite};
use crate::world::block::Block;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconTex {
    Miner,
    Pipe,
    StorageTank,
    WaterPump,
    Electrolyzer,
    Centrifuge,
    FuelFabricator,
    Boiler,
    GeothermalWell,
    WindTurbine,
    Rtg,
    Blender,
    SteamTurbine,
    NuclearReactor,
    CoolingPool,
    ReprocessingPlant,
    ParticleAccelerator,
}

impl IconTex {
    pub const ALL: &'static [IconTex] = &[
        IconTex::Miner,
        IconTex::Pipe,
        IconTex::StorageTank,
        IconTex::WaterPump,
        IconTex::Electrolyzer,
        IconTex::Centrifuge,
        IconTex::FuelFabricator,
        IconTex::Boiler,
        IconTex::GeothermalWell,
        IconTex::WindTurbine,
        IconTex::Rtg,
        IconTex::Blender,
        IconTex::SteamTurbine,
        IconTex::NuclearReactor,
        IconTex::CoolingPool,
        IconTex::ReprocessingPlant,
        IconTex::ParticleAccelerator,
    ];

    /// Its layer in the texture array: after every `Tex`.
    pub fn layer(self) -> u32 {
        (Tex::ALL.len() + self as usize) as u32
    }

    /// The picture a block's item shows, if it has one.
    pub fn of(block: Block) -> Option<IconTex> {
        Some(match block {
            Block::Miner => IconTex::Miner,
            Block::Pipe => IconTex::Pipe,
            Block::StorageTank => IconTex::StorageTank,
            Block::WaterPump => IconTex::WaterPump,
            Block::Electrolyzer => IconTex::Electrolyzer,
            Block::Centrifuge => IconTex::Centrifuge,
            Block::FuelFabricator => IconTex::FuelFabricator,
            Block::Boiler => IconTex::Boiler,
            Block::GeothermalWell => IconTex::GeothermalWell,
            Block::WindTurbine => IconTex::WindTurbine,
            Block::Rtg => IconTex::Rtg,
            Block::Blender => IconTex::Blender,
            Block::SteamTurbine => IconTex::SteamTurbine,
            Block::NuclearReactor => IconTex::NuclearReactor,
            Block::CoolingPool => IconTex::CoolingPool,
            Block::ReprocessingPlant => IconTex::ReprocessingPlant,
            Block::ParticleAccelerator => IconTex::ParticleAccelerator,
            _ => return None,
        })
    }
}

const STEEL: Rgb = [150, 158, 174];
const DARK: Rgb = [70, 76, 88];
const SHELL: Rgb = [46, 52, 66];
const PAINT: Rgb = [222, 224, 216];
const RIG: Rgb = [220, 164, 40];
const STRIPE: Rgb = [40, 40, 44];
const CONCRETE: Rgb = [158, 158, 152];
const WATER: Rgb = [60, 130, 230];
const CHERENKOV: Rgb = [110, 200, 255];
const FIRE: Rgb = [255, 150, 50];
const FLAME: Rgb = [255, 226, 120];
const LAVA: Rgb = [255, 96, 30];
const STEAM: Rgb = [232, 236, 240];
const LAMP: Rgb = [110, 240, 120];
const RED: Rgb = [235, 70, 60];
const URANIUM: Rgb = [150, 220, 60];
const SCREEN: Rgb = [56, 196, 250];
const VIOLET: Rgb = [176, 110, 255];

/// A 16 x 16 picture being painted (later shapes over earlier ones).
struct Paint([[Option<Rgb>; 16]; 16]);

impl Paint {
    fn new() -> Self {
        Paint([[None; 16]; 16])
    }

    fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if (0..16).contains(&x) && (0..16).contains(&y) {
            self.0[y as usize][x as usize] = Some(c);
        }
    }

    /// A box from (x0, y0) to (x1, y1), inclusive: its top and left edges lit, its bottom and
    /// right in shade.
    fn rect(&mut self, [x0, y0, x1, y1]: [i32; 4], c: Rgb) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let f = if y == y0 || x == x0 {
                    1.22
                } else if y == y1 || x == x1 {
                    0.72
                } else {
                    1.0
                };
                self.set(x, y, scale(c, f));
            }
        }
    }

    /// A box all one colour.
    fn flat(&mut self, [x0, y0, x1, y1]: [i32; 4], c: Rgb) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.set(x, y, c);
            }
        }
    }

    /// A standing cylinder: light on the left, shade on the right.
    fn column(&mut self, [x0, y0, x1, y1]: [i32; 4], c: Rgb) {
        for x in x0..=x1 {
            let t = (x - x0) as f32 / (x1 - x0).max(1) as f32;
            for y in y0..=y1 {
                let top = if y == y0 { 1.12 } else { 1.0 };
                self.set(x, y, scale(c, (1.25 - 0.6 * t) * top));
            }
        }
    }

    /// A lying cylinder: light on top, shade below.
    fn lying(&mut self, [x0, y0, x1, y1]: [i32; 4], c: Rgb) {
        for y in y0..=y1 {
            let t = (y - y0) as f32 / (y1 - y0).max(1) as f32;
            for x in x0..=x1 {
                self.set(x, y, scale(c, 1.25 - 0.6 * t));
            }
        }
    }

    /// A disc about (cx, cy) (pixel centres at .5), lit from the top left.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: Rgb) {
        for y in 0..16 {
            for x in 0..16 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy < r * r {
                    self.set(x, y, scale(c, 1.0 - 0.2 * (dx + dy) / r.max(1.0)));
                }
            }
        }
    }

    /// A ring about (cx, cy) between radii `inner` and `outer`.
    fn ring(&mut self, cx: f32, cy: f32, inner: f32, outer: f32, c: Rgb) {
        for y in 0..16 {
            for x in 0..16 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if (inner..outer).contains(&d) {
                    self.set(x, y, scale(c, 1.0 - 0.22 * (dx + dy) / outer));
                }
            }
        }
    }

    /// A line one pixel wide from (x0, y0) to (x1, y1).
    fn line(&mut self, (x0, y0): (i32, i32), (x1, y1): (i32, i32), c: Rgb) {
        let n = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = x0 as f32 + (x1 - x0) as f32 * t;
            let y = y0 as f32 + (y1 - y0) as f32 * t;
            self.set(x.round() as i32, y.round() as i32, c);
        }
    }

    /// Yellow and black stripes along a box.
    fn hazard(&mut self, [x0, y0, x1, y1]: [i32; 4]) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let c = if (x + y).rem_euclid(4) < 2 {
                    RIG
                } else {
                    STRIPE
                };
                self.set(x, y, c);
            }
        }
    }

    fn img(self) -> Img {
        sprite(|x, y| self.0[y as usize][x as usize])
    }
}

pub fn make(icon: IconTex) -> Img {
    let mut p = Paint::new();
    match icon {
        IconTex::Miner => {
            // A derrick on its deck, the drill down the middle.
            p.hazard([0, 14, 15, 15]);
            for (a, b) in [((2, 13), (6, 1)), ((13, 13), (9, 1))] {
                p.line(a, b, RIG);
            }
            for (a, b) in [
                ((3, 10), (12, 10)),
                ((4, 6), (11, 6)),
                ((5, 3), (10, 3)),
                ((3, 10), (11, 6)),
                ((4, 6), (10, 3)),
            ] {
                p.line(a, b, scale(RIG, 0.85));
            }
            p.flat([7, 2, 8, 13], DARK);
            p.rect([5, 0, 10, 1], STEEL);
            p.set(5, 0, RED);
            p.rect([10, 11, 14, 13], STEEL);
            p.flat([11, 12, 13, 12], SCREEN);
        }
        IconTex::Pipe => {
            // A length of pipe between flanges, a valve on top.
            p.lying([1, 6, 14, 10], STEEL);
            p.rect([0, 5, 1, 11], scale(STEEL, 0.85));
            p.rect([14, 5, 15, 11], scale(STEEL, 0.85));
            p.flat([7, 3, 8, 5], DARK);
            p.rect([4, 2, 11, 3], RED);
        }
        IconTex::StorageTank => {
            // A white tank on legs, a gauge down its side showing water.
            p.flat([4, 14, 5, 15], DARK);
            p.flat([10, 14, 11, 15], DARK);
            p.column([2, 2, 13, 13], PAINT);
            p.column([4, 1, 11, 1], PAINT);
            p.flat([3, 6, 12, 6], scale(PAINT, 0.8));
            p.flat([10, 3, 11, 12], SHELL);
            p.flat([10, 8, 11, 12], WATER);
        }
        IconTex::WaterPump => {
            // A pump housing with its motor, the intake into the water, the outlet off the side.
            p.flat([6, 12, 7, 13], STEEL);
            p.flat([1, 14, 14, 15], WATER);
            p.flat([3, 14, 4, 14], scale(WATER, 1.3));
            p.flat([10, 15, 12, 15], scale(WATER, 1.3));
            p.rect([2, 5, 11, 11], STEEL);
            p.rect([4, 1, 9, 4], DARK);
            p.lying([12, 6, 15, 8], STEEL);
            p.flat([4, 7, 5, 8], LAMP);
        }
        IconTex::Electrolyzer => {
            // A cell with a hydrogen and an oxygen column on top, a bolt on its front.
            p.column([2, 1, 5, 7], [120, 170, 255]);
            p.column([10, 1, 13, 7], [255, 110, 100]);
            p.rect([1, 7, 14, 14], STEEL);
            for (x, y) in [
                (8, 8),
                (7, 9),
                (6, 10),
                (7, 10),
                (8, 10),
                (9, 10),
                (8, 11),
                (7, 12),
            ] {
                p.set(x, y, FLAME);
            }
        }
        IconTex::Centrifuge => {
            // A tall white cylinder banded in steel, on a dark plinth.
            p.column([4, 0, 11, 13], PAINT);
            for y in [3, 7, 11] {
                p.flat([4, y, 11, y], scale(STEEL, 0.9));
            }
            p.set(9, 5, LAMP);
            p.rect([2, 14, 13, 15], DARK);
        }
        IconTex::FuelFabricator => {
            // A machine with its screen, a finished fuel rod standing beside it.
            p.rect([0, 4, 10, 15], STEEL);
            p.flat([2, 6, 7, 9], SCREEN);
            p.flat([2, 12, 8, 12], DARK);
            p.column([12, 1, 14, 15], STEEL);
            for y in (3..=13).step_by(2) {
                p.set(13, y, URANIUM);
            }
        }
        IconTex::Boiler => {
            // A drum over a firebox, its fire showing, steam off the top.
            p.rect([2, 8, 13, 15], DARK);
            p.flat([5, 10, 10, 13], FIRE);
            p.flat([6, 11, 9, 13], FLAME);
            p.lying([1, 4, 14, 7], PAINT);
            p.disc(11.0, 2.0, 2.0, STEAM);
            p.disc(13.5, 1.0, 1.4, STEAM);
        }
        IconTex::GeothermalWell => {
            // A wellhead over cracked ground glowing with lava, steam rising from it.
            p.flat([0, 13, 15, 15], [70, 60, 58]);
            for (x, y) in [
                (2, 14),
                (3, 13),
                (4, 14),
                (9, 15),
                (10, 14),
                (12, 13),
                (13, 14),
            ] {
                p.set(x, y, LAVA);
            }
            p.column([6, 6, 9, 12], STEEL);
            p.rect([4, 8, 11, 9], DARK);
            p.disc(7.5, 3.5, 2.6, STEAM);
            p.disc(10.0, 1.6, 1.6, STEAM);
        }
        IconTex::WindTurbine => {
            // A tall tower, its rotor of three blades.
            p.column([7, 7, 8, 15], PAINT);
            p.line((7, 5), (7, 0), PAINT);
            p.line((6, 7), (2, 10), PAINT);
            p.line((9, 7), (13, 10), scale(PAINT, 0.85));
            p.rect([6, 5, 9, 7], STEEL);
            p.flat([7, 6, 7, 6], DARK);
        }
        IconTex::Rtg => {
            // A dark finned core with a yellow warning band.
            for y in (3..=13).step_by(2) {
                p.flat([2, y, 13, y], scale(DARK, 0.9));
            }
            p.column([5, 1, 10, 14], DARK);
            p.flat([5, 7, 10, 8], RIG);
            p.flat([7, 7, 8, 8], STRIPE);
            p.flat([6, 0, 9, 0], STEEL);
        }
        IconTex::Blender => {
            // A big white vat banded in steel, its motor on top, on a striped base.
            p.hazard([1, 14, 14, 15]);
            p.column([2, 4, 13, 13], PAINT);
            for y in [7, 10] {
                p.flat([2, y, 13, y], scale(STEEL, 0.85));
            }
            p.rect([5, 1, 10, 3], STEEL);
            p.flat([7, 0, 8, 0], DARK);
            p.set(11, 5, LAMP);
        }
        IconTex::SteamTurbine => {
            // The turbine casing lying on its side, coupled to the generator; steam in on top.
            p.flat([3, 1, 4, 5], STEEL);
            p.lying([0, 5, 8, 12], STEEL);
            p.flat([1, 6, 1, 11], scale(STEEL, 0.75));
            p.flat([9, 8, 9, 9], DARK);
            p.rect([10, 4, 15, 13], PAINT);
            p.rect([0, 13, 15, 15], DARK);
        }
        IconTex::NuclearReactor => {
            // A concrete dome on its building, a red light on top, striped at its foot.
            p.rect([1, 9, 14, 13], CONCRETE);
            for y in 2..=9 {
                for x in 0..16 {
                    let (dx, dy) = (x as f32 + 0.5 - 8.0, y as f32 + 0.5 - 9.5);
                    if dx * dx + dy * dy < 6.6 * 6.6 {
                        p.set(x, y, scale(CONCRETE, 1.12 - 0.25 * (dx + dy) / 6.6));
                    }
                }
            }
            p.flat([7, 2, 8, 2], RED);
            p.flat([3, 10, 5, 12], SHELL);
            p.flat([4, 11, 4, 11], SCREEN);
            p.hazard([1, 14, 14, 15]);
        }
        IconTex::CoolingPool => {
            // A concrete pool full of glowing blue water, the racks of fuel at its bottom.
            p.rect([0, 4, 15, 13], CONCRETE);
            p.flat([2, 6, 13, 11], CHERENKOV);
            for x in [4, 7, 10] {
                p.flat([x, 7, x, 11], scale(CHERENKOV, 0.7));
            }
            p.flat([3, 6, 6, 6], [200, 240, 255]);
            p.flat([9, 8, 11, 8], [200, 240, 255]);
        }
        IconTex::ReprocessingPlant => {
            // A concrete block with the amber windows of its hot cells, a stack on top.
            p.column([11, 0, 12, 5], STEEL);
            p.rect([0, 5, 15, 15], CONCRETE);
            for x in [2, 7, 12] {
                p.flat([x, 8, x + 1, 10], [255, 196, 90]);
            }
            p.flat([0, 12, 15, 12], scale(CONCRETE, 0.8));
            p.flat([6, 13, 9, 15], SHELL);
        }
        IconTex::ParticleAccelerator => {
            // The ring of magnets, the beam glowing round it, the target in the middle.
            p.ring(8.0, 8.0, 4.2, 7.5, STEEL);
            for (x, y) in [
                (7, 0),
                (7, 14),
                (0, 7),
                (14, 7),
                (2, 2),
                (12, 2),
                (2, 12),
                (12, 12),
            ] {
                p.flat([x, y, x + 1, y + 1], scale(SHELL, 1.3));
            }
            p.ring(8.0, 8.0, 5.4, 6.3, VIOLET);
            p.rect([6, 6, 9, 9], DARK);
            p.flat([7, 7, 8, 8], [228, 196, 255]);
        }
    }
    p.img()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_big_machine_has_a_picture() {
        for (i, &icon) in IconTex::ALL.iter().enumerate() {
            assert_eq!(
                icon as usize, i,
                "IconTex::ALL must be in declaration order"
            );
            let blocks = Block::ALL
                .iter()
                .filter(|&&b| IconTex::of(b) == Some(icon))
                .count();
            assert_eq!(blocks, 1, "{icon:?} is the picture of {blocks} blocks");
            let shown = make(icon).px.iter().filter(|p| p[3] != 0).count();
            assert!(shown > 40, "{icon:?} is nearly empty ({shown} pixels)");
            // (The interface has 12 bits for a layer: `ui::FONT_LAYER`.)
            assert!(icon.layer() < 4096);
        }
        for &b in Block::ALL.iter() {
            if b.is_structure() {
                assert!(IconTex::of(b).is_some(), "{b:?} has no picture");
            }
        }
    }

    /// Writes every picture, eight times its size, to `target/icons.bmp` (to look at them).
    #[test]
    #[ignore]
    fn write_icons() {
        let images: Vec<_> = IconTex::ALL.iter().map(|&i| make(i).px).collect();
        super::super::write_sheet("target/icons.bmp", &images);
    }
}
