//! Textures for chemistry and nuclear power: the new ores and building blocks, the big
//! machines' panels, fluids (a flask for a liquid, a cylinder for a gas, coloured by
//! `Fluid::color`), and what the chains make. The game's own look, like `industry.rs`: flat
//! shapes with a lit top-left edge.

use super::{
    Img, Rgb, Rng, TEX_SIZE, Tex, ingot, lump, noisy, panel, scale, speckle, sprite, stone,
};
use crate::factory::fluids::Fluid;

const INK: Rgb = [20, 23, 31];
const STEEL: Rgb = [150, 158, 174];
const STEEL_HI: Rgb = [214, 220, 232];
const SHELL: Rgb = [46, 52, 66];
const URANIUM: Rgb = [150, 220, 60];
const GLOW_BLUE: Rgb = [120, 200, 255];

/// A heap of powder (`base`, lighter on top) on the bottom of the sprite.
fn pile(r: &mut Rng, base: Rgb) -> Img {
    let noise: Vec<f32> = (0..256).map(|_| r.signed()).collect();
    sprite(|x, y| {
        let dx = (x - 8.0) / 6.5;
        let top = 13.5 - 7.5 * (1.0 - dx * dx).max(0.0).sqrt();
        if y < top || y > 13.5 || dx.abs() > 1.0 {
            return None;
        }
        let i = (y as usize) * 16 + x as usize;
        let light = 1.15 - (y - top) * 0.05 - dx * 0.12;
        Some(scale(base, light + noise[i] * 0.07))
    })
}

/// A fuel rod lying corner to corner: a steel tube with end caps, `band` the pellets showing
/// through its window, `glow` round it (hot spent fuel), if any.
fn rod(band: Rgb, glow: Option<Rgb>) -> Img {
    sprite(|x, y| {
        // Distance along the rod (from bottom left to top right) and across it.
        let (px, py) = (x - 7.5, y - 7.5);
        let along = (px - py) * std::f32::consts::FRAC_1_SQRT_2;
        let across = (px + py) * std::f32::consts::FRAC_1_SQRT_2;
        let a = across.abs();
        if along.abs() > 8.6 {
            return None;
        }
        if a > 2.2 {
            return glow.filter(|_| a < 3.2 && along.abs() < 7.5);
        }
        if along.abs() > 7.4 {
            return Some(scale(STEEL, 0.75));
        }
        if a < 1.0 && along.abs() < 5.5 && (along * 1.4).rem_euclid(2.0) > 0.4 {
            return Some(band);
        }
        Some(if across < -0.9 {
            STEEL_HI
        } else if across > 1.1 {
            scale(STEEL, 0.7)
        } else {
            STEEL
        })
    })
}

/// A round-bottomed flask holding a liquid of `color`.
fn flask(color: Rgb) -> Img {
    sprite(|x, y| {
        let (x, y) = (x.floor(), y.floor());
        let (dx, dy) = (x - 7.5, y - 9.5);
        let bulb = dx * dx + dy * dy < 24.0;
        let neck = (6.0..=9.0).contains(&x) && (2.0..7.0).contains(&y);
        let lip = (5.0..=10.0).contains(&x) && (1.0..2.0).contains(&y);
        if !(bulb || neck || lip) {
            return None;
        }
        let edge = bulb && dx * dx + dy * dy > 17.0;
        if lip || (neck && (x == 6.0 || x == 9.0)) || edge {
            return Some([196, 214, 226]);
        }
        if bulb && y >= 8.0 {
            let shine = dx < -1.0 && dy < 0.5;
            return Some(if shine {
                scale(color, 1.3)
            } else {
                scale(color, 1.0 - dy * 0.05)
            });
        }
        Some([110, 130, 150])
    })
}

/// A gas cylinder: a steel bottle with a band and shoulder of `color`, and a valve on top.
fn cylinder(color: Rgb) -> Img {
    sprite(|x, y| {
        let (x, y) = (x as i32, y as i32);
        if (7..=8).contains(&x) && (0..=1).contains(&y) {
            return Some(INK);
        }
        if (6..=9).contains(&x) && y == 2 {
            return Some(STEEL);
        }
        let body =
            (4..=11).contains(&x) && (4..=15).contains(&y) || (5..=10).contains(&x) && y == 3;
        if !body {
            return None;
        }
        let shade = if x == 4 || x == 5 {
            1.25
        } else if x >= 10 {
            0.7
        } else {
            1.0
        };
        let c = if y <= 6 || (10..=11).contains(&y) {
            color
        } else {
            [128, 134, 146]
        };
        Some(scale(c, shade))
    })
}

/// Grains of sand (`base`) heaped up.
fn grains(r: &mut Rng, base: Rgb, fleck: Rgb) -> Img {
    let mut img = pile(r, base);
    for _ in 0..22 {
        let (x, y) = (2 + r.below(12), 6 + r.below(8));
        if img.get(x, y)[3] != 0 {
            img.set(x, y, fleck, 255);
        }
    }
    img
}

pub(super) fn make(t: Tex, r: &mut Rng) -> Img {
    if let Some(fluid) = Fluid::ALL.iter().find(|f| f.tex() == t) {
        return if fluid.is_gas() {
            cylinder(fluid.color())
        } else {
            flask(fluid.color())
        };
    }
    match t {
        Tex::SulfurOre => {
            let mut img = stone(r);
            speckle(
                r,
                &mut img,
                6,
                &[[236, 214, 60], [250, 236, 120], [200, 170, 40]],
                0,
            );
            img
        }
        Tex::LeadOre => {
            let mut img = stone(r);
            // Galena: little bright grey cubes.
            for _ in 0..5 {
                let (x, y) = (1 + r.below(13), 1 + r.below(13));
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let c = if dx == 0 && dy == 0 {
                        [190, 196, 214]
                    } else {
                        [104, 110, 130]
                    };
                    img.set(x + dx, y + dy, c, 0);
                }
            }
            img
        }
        Tex::FluoriteOre => {
            let mut img = stone(r);
            speckle(
                r,
                &mut img,
                5,
                &[[150, 80, 220], [200, 140, 250], [90, 200, 160]],
                0,
            );
            img
        }
        Tex::SaltpeterOre => {
            let mut img = noisy(r, [214, 190, 150], 0.06, 0);
            speckle(r, &mut img, 9, &[[246, 244, 236], [230, 228, 220]], 0);
            img
        }
        Tex::Concrete => {
            let mut img = noisy(r, [158, 158, 152], 0.035, 0);
            speckle(r, &mut img, 5, &[[130, 130, 126], [176, 176, 170]], 0);
            for i in 0..TEX_SIZE {
                img.shade(i, 15, 0.86);
                img.shade(15, i, 0.86);
            }
            for (x, y) in [(3, 3), (12, 3), (3, 12), (12, 12)] {
                img.set(x, y, [120, 120, 118], 0);
            }
            img
        }
        Tex::SteelPlate => {
            // Tread plate: raised diagonal bars.
            let mut img = panel(r, [138, 144, 156]);
            for y in 1..15 {
                for x in 1..15 {
                    let (cx, cy) = (x % 5, y % 5);
                    let up = (x / 5 + y / 5) % 2 == 0;
                    if (up && cx == cy && cx > 0) || (!up && cx + cy == 4 && cx > 0 && cy > 0) {
                        img.set(x, y, [196, 202, 214], 0);
                    }
                }
            }
            img
        }
        Tex::VitrifiedSide => {
            // A steel can with a hazard band and a strip of green glass.
            let mut img = panel(r, [120, 126, 136]);
            for x in 0..TEX_SIZE {
                for y in 3..=4 {
                    let stripe = (x + y) % 4 < 2;
                    img.set(x, y, if stripe { [236, 196, 40] } else { INK }, 0);
                }
                for y in 8..=11 {
                    img.set(x, y, scale([90, 220, 120], 0.8 + (x % 3) as f32 * 0.1), 0);
                }
            }
            img
        }
        Tex::VitrifiedTop => {
            let mut img = panel(r, [120, 126, 136]);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 4.5 {
                        img.set(x, y, scale([110, 240, 140], 1.1 - d * 0.08), 0);
                    } else if d < 5.5 {
                        img.set(x, y, [80, 84, 92], 0);
                    }
                }
            }
            img
        }
        Tex::ControlPanel | Tex::ControlPanelLit => {
            let lit = t == Tex::ControlPanelLit;
            let mut img = noisy(r, [58, 64, 78], 0.03, 0);
            let screen = if lit { [90, 230, 200] } else { [26, 34, 40] };
            let trace = if lit { [200, 255, 240] } else { [40, 52, 58] };
            for y in 2..=6 {
                for x in 1..=6 {
                    img.set(x, y, screen, 0);
                }
                for x in 9..=14 {
                    img.set(x, y, screen, 0);
                }
            }
            for x in 1..=6 {
                img.set(x, 6 - (x % 4), trace, 0);
            }
            for x in 9..=14 {
                img.set(x, 4 + (x % 2), trace, 0);
            }
            let colors = [[240, 80, 60], [250, 200, 60], [90, 230, 110]];
            for (i, x) in [2usize, 5, 8, 11, 13].iter().enumerate() {
                let c = colors[i % 3];
                img.set(*x, 10, if lit { c } else { scale(c, 0.3) }, 0);
            }
            for x in 1..15 {
                img.set(x, 13, [36, 40, 50], 0);
            }
            img
        }
        Tex::Cherenkov => {
            // Deep water lit blue from below, with ripples.
            let mut img = noisy(r, [40, 120, 255], 0.06, 0);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let wave = ((x as f32 * 0.8 + y as f32 * 0.35).sin()
                        + (y as f32 * 0.9 - x as f32 * 0.2).cos())
                        * 0.5;
                    if wave > 0.75 {
                        img.set(x, y, GLOW_BLUE, 0);
                    }
                }
            }
            img
        }
        Tex::HotCell | Tex::HotCellLit => {
            // A thick window of lead glass in a concrete wall: dark, or lit amber inside with a
            // manipulator's arm.
            let lit = t == Tex::HotCellLit;
            let mut img = noisy(r, [150, 150, 146], 0.035, 0);
            let inside = if lit { [255, 196, 90] } else { [40, 44, 40] };
            for y in 3..=12 {
                for x in 2..=13 {
                    let frame = y == 3 || y == 12 || x == 2 || x == 13;
                    img.set(x, y, if frame { [70, 74, 80] } else { inside }, 0);
                }
            }
            if lit {
                for i in 0..6 {
                    img.set(5 + i, 5 + i / 2, [90, 70, 40], 0);
                }
                img.set(10, 8, [90, 70, 40], 0);
                img.set(11, 9, [90, 70, 40], 0);
            }
            img
        }
        Tex::TankPaint => {
            let mut img = noisy(r, [222, 224, 216], 0.025, 0);
            for y in 0..TEX_SIZE {
                img.shade(0, y, 0.8);
                img.shade(15, y, 0.8);
            }
            for x in 0..TEX_SIZE {
                img.shade(x, 7, 0.88);
            }
            for (x, y) in [(2, 1), (13, 1), (2, 14), (13, 14)] {
                img.shade(x, y, 0.7);
            }
            img
        }
        // (Even all over: a pipe's faces take any slice of them, along any axis.)
        Tex::PipeSteel => {
            let mut img = noisy(r, [176, 182, 190], 0.025, 0);
            speckle(r, &mut img, 5, &[[196, 202, 210], [150, 156, 166]], 0);
            img
        }
        Tex::PipeFitting => {
            let mut img = noisy(r, [92, 98, 108], 0.04, 0);
            speckle(r, &mut img, 6, &[[120, 126, 136], [70, 74, 82]], 0);
            img
        }
        Tex::AntimatterFace => {
            let mut img = noisy(r, [40, 36, 52], 0.04, 0);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 2.0 {
                        img.set(x, y, [255, 230, 255], 0);
                    } else if d < 4.5 {
                        img.set(x, y, [180, 90, 255], 0);
                    } else if (5.5..6.6).contains(&d) {
                        img.set(x, y, [210, 160, 255], 0);
                    }
                }
            }
            img
        }
        Tex::Sulfur => lump(r, [228, 206, 60], [255, 246, 160], 5.5),
        Tex::Fluorite => {
            // A cube crystal, violet with a green edge.
            sprite(|x, y| {
                let (x, y) = (x as i32, y as i32);
                let top = (3..=6).contains(&y) && (6 - (y - 3)..=12 - (y - 3)).contains(&x);
                let front = (7..=13).contains(&y) && (3..=9).contains(&x);
                let side = (4..=12).contains(&y) && (10..=12).contains(&x) && x - 10 <= y - 4;
                if top {
                    Some([214, 170, 255])
                } else if front {
                    Some(if x == 3 || y == 13 {
                        [100, 210, 170]
                    } else {
                        [150, 84, 220]
                    })
                } else if side {
                    Some([100, 50, 160])
                } else {
                    None
                }
            })
        }
        Tex::Saltpeter => lump(r, [236, 232, 222], [255, 255, 255], 5.0),
        Tex::RawLead => lump(r, [96, 102, 118], [200, 206, 222], 5.0),
        Tex::LeadIngot => ingot([104, 110, 126]),
        Tex::Alumina => pile(r, [238, 236, 230]),
        Tex::AluminiumIngot => ingot([204, 210, 218]),
        Tex::TitaniumIngot => ingot([164, 158, 176]),
        Tex::Zircon => grains(r, [176, 110, 78], [236, 200, 170]),
        Tex::Zirconium => ingot([176, 186, 172]),
        Tex::ElectricMotor => sprite(|x, y| {
            let (x, y) = (x as i32, y as i32);
            // A drum on its side: copper windings showing at the end, a shaft out of it.
            if (12..=15).contains(&x) && (7..=8).contains(&y) {
                return Some(STEEL_HI);
            }
            if (2..=11).contains(&x) && (3..=12).contains(&y) {
                let end = x <= 3;
                return Some(if end {
                    if (y + x) % 2 == 0 {
                        [226, 132, 78]
                    } else {
                        [170, 90, 50]
                    }
                } else if y == 3 {
                    STEEL_HI
                } else if y >= 11 {
                    scale(STEEL, 0.7)
                } else if x % 2 == 0 {
                    STEEL
                } else {
                    scale(STEEL, 0.85)
                });
            }
            ((3..=10).contains(&x) && (13..=14).contains(&y)).then_some(SHELL)
        }),
        Tex::TurbineBlade => sprite(|x, y| {
            // A long twisted blade with its root at the bottom left.
            let (px, py) = (x - 7.5, y - 7.5);
            let along = (px - py) * std::f32::consts::FRAC_1_SQRT_2;
            let across = (px + py) * std::f32::consts::FRAC_1_SQRT_2;
            let width = 1.0 + (along + 8.0) * 0.12;
            if along < -7.5 || along > 8.0 || across.abs() > width {
                return None;
            }
            if along < -5.5 {
                return Some(SHELL);
            }
            Some(if across < 0.0 {
                [200, 196, 214]
            } else {
                [140, 136, 156]
            })
        }),
        Tex::Magnet => sprite(|x, y| {
            // A C-shaped yoke (blue) wound with copper.
            let (dx, dy) = (x - 7.5, y - 7.5);
            let d = (dx * dx + dy * dy).sqrt();
            let gap = dx > 2.0 && dy.abs() < 2.0;
            if !(3.5..7.0).contains(&d) || gap {
                return None;
            }
            let coil = dy.atan2(dx).abs() > 2.2;
            Some(if coil {
                if (x as i32 + y as i32) % 2 == 0 {
                    [226, 132, 78]
                } else {
                    [180, 96, 56]
                }
            } else if d < 4.3 {
                [60, 90, 170]
            } else {
                [90, 130, 220]
            })
        }),
        Tex::Yellowcake => pile(r, [236, 196, 40]),
        Tex::DepletedUranium => ingot([84, 90, 84]),
        Tex::UraniumFuelRod => rod(URANIUM, None),
        Tex::MoxFuelRod => rod([250, 140, 60], None),
        Tex::SpentFuelRod => rod([90, 110, 120], Some(GLOW_BLUE)),
        Tex::CooledSpentFuel => rod([90, 96, 100], None),
        Tex::FuelHulls => sprite(|x, y| {
            // Short pieces of chopped tube.
            let (x, y) = (x as i32, y as i32);
            let pieces = [(2, 9), (8, 11), (5, 4), (10, 5)];
            pieces.iter().find_map(|&(px, py)| {
                let (dx, dy) = (x - px, y - py);
                let color = if dx == 0 {
                    INK
                } else if dy == 0 {
                    STEEL_HI
                } else {
                    STEEL
                };
                ((0..=3).contains(&dx) && (0..=2).contains(&dy)).then_some(color)
            })
        }),
        Tex::RecoveredUranium => pile(r, [70, 96, 60]),
        Tex::Plutonium => sprite(|x, y| {
            // A stack of three pellets, warm to look at.
            let (x, y) = (x as i32, y as i32);
            let pellet = (5..=10).contains(&x) && (2..=14).contains(&y) && y % 4 != 1;
            if pellet {
                return Some(if x == 5 {
                    [222, 220, 230]
                } else if x == 10 {
                    [120, 116, 128]
                } else {
                    [176, 172, 184]
                });
            }
            ((4..=11).contains(&x) && (1..=15).contains(&y)).then_some([200, 90, 60])
        }),
        Tex::Antimatter => sprite(|x, y| {
            // A magnetic bottle: a glass capsule between two caps, a violet spark held inside.
            let (x, y) = (x as i32, y as i32);
            if (4..=11).contains(&x) && ((1..=3).contains(&y) || (12..=14).contains(&y)) {
                return Some(if x == 4 { STEEL_HI } else { STEEL });
            }
            if (5..=10).contains(&x) && (4..=11).contains(&y) {
                let (dx, dy) = (x as f32 - 7.5, y as f32 - 7.5);
                let d = (dx * dx + dy * dy).sqrt();
                return Some(if d < 1.6 {
                    [255, 236, 255]
                } else if d < 3.0 {
                    [190, 100, 255]
                } else {
                    [70, 60, 100]
                });
            }
            None
        }),
        _ => noisy(r, [255, 0, 255], 0.0, 0),
    }
}
