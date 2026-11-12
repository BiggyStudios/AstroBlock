//! Textures for industry: oil shale, the machines' fronts (idle and working), the paint and
//! lamps of their models, and what factories make.
//!
//! The electronics are drawn as the game's own parts, not pictures of real ones: dark graphite
//! shells with a lit top-left edge, and what makes each one itself glowing (cyan for data and
//! light, amber for computing and power, violet for AI, mint for charge).

use super::{Img, Rgb, Rng, TEX_SIZE, Tex, ingot, lump, noisy, panel, scale, speckle, sprite};

const INK: Rgb = [20, 23, 31];
const SHELL: Rgb = [46, 52, 66];
const SHELL_HI: Rgb = [84, 94, 116];
const STEEL: Rgb = [150, 158, 174];
const STEEL_HI: Rgb = [214, 220, 232];
const CYAN: Rgb = [56, 196, 250];
const CYAN_HI: Rgb = [196, 246, 255];
const AMBER: Rgb = [255, 160, 44];
const AMBER_HI: Rgb = [255, 234, 170];
const VIOLET: Rgb = [160, 88, 255];
const VIOLET_HI: Rgb = [228, 196, 255];
const MINT: Rgb = [70, 232, 150];
const COPPER: Rgb = [226, 132, 78];
const GOLD: Rgb = [232, 190, 76];

/// A machine's status lamp (top right of its front): green while it works.
fn lamp(img: &mut Img, lit: bool) {
    let color = if lit { [110, 240, 120] } else { [40, 54, 44] };
    for (x, y) in [(12, 2), (13, 2), (12, 3), (13, 3)] {
        img.set(x, y, color, 0);
    }
}

/// Fills a rectangle (inclusive pixel ranges) with `color`, a little noisy.
fn fill(
    img: &mut Img,
    r: &mut Rng,
    xs: std::ops::RangeInclusive<usize>,
    ys: std::ops::RangeInclusive<usize>,
    color: Rgb,
) {
    for y in ys {
        for x in xs.clone() {
            img.set(x, y, scale(color, 1.0 + r.signed() * 0.04), 0);
        }
    }
}

/// `base` on a box from (x0, y0) to (x1, y1) (inclusive): its top and left edges catch the
/// light, its bottom and right are in shade.
fn bevel(x: i32, y: i32, [x0, y0, x1, y1]: [i32; 4], base: Rgb) -> Rgb {
    if y == y0 || x == x0 {
        scale(base, 1.45)
    } else if y == y1 || x == x1 {
        scale(base, 0.62)
    } else {
        base
    }
}

/// Whether (x, y) is inside the box (inclusive).
fn inside(x: i32, y: i32, [x0, y0, x1, y1]: [i32; 4]) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// A spool of wire seen from the side: two dark flanges, the windings between them (`wire`
/// and its shade by turns, `shine` where the light catches them), the end hanging off.
fn spool(wire: Rgb, shine: Rgb, end: Rgb) -> Img {
    sprite(|x, y| {
        let (x, y) = (x as i32, y as i32);
        if (x == 2 || x == 3 || x == 12 || x == 13) && (1..=14).contains(&y) {
            return Some(if y == 1 {
                SHELL_HI
            } else if x == 2 || x == 12 {
                scale(SHELL_HI, 0.85)
            } else {
                SHELL
            });
        }
        if (4..=11).contains(&x) && (3..=12).contains(&y) {
            return Some(if y == 4 && (5..=9).contains(&x) {
                shine
            } else if y == 12 {
                scale(wire, 0.55)
            } else if y % 2 == 0 {
                wire
            } else {
                scale(wire, 0.74)
            });
        }
        // The loose end, coming off the bottom.
        matches!((x, y), (9, 13) | (10, 14) | (11, 14) | (12, 15)).then_some(end)
    })
}

/// A chip seen from above: pins round a bevelled `body` (`pin` coloured), with `die` drawing
/// its middle (given (x, y), returning a colour there or nothing).
fn chip(body: Rgb, pin: Rgb, die: impl Fn(i32, i32) -> Option<Rgb>) -> Img {
    sprite(|x, y| {
        let (x, y) = (x as i32, y as i32);
        let edge = [2, 2, 13, 13];
        if inside(x, y, edge) {
            return Some(die(x, y).unwrap_or_else(|| bevel(x, y, edge, body)));
        }
        let pins = |a: i32, b: i32| (a == 1 || a == 14) && (3..=12).contains(&b) && b % 2 == 1;
        (pins(x, y) || pins(y, x)).then_some(pin)
    })
}

pub(super) fn make(t: Tex, r: &mut Rng) -> Img {
    let lit = matches!(
        t,
        Tex::MinerFrontLit
            | Tex::PumpFrontLit
            | Tex::RefineryFrontLit
            | Tex::ArcFrontLit
            | Tex::AssemblerFrontLit
            | Tex::FabFrontLit
            | Tex::AccumulatorFrontLit
    );
    match t {
        Tex::OilShale => {
            // Dark layered rock with glossy black seams of oil.
            let mut img = noisy(r, [64, 58, 52], 0.1, 0);
            for _ in 0..5 {
                let (x0, y) = (r.below(10), r.below(16));
                let end = (x0 + 4 + r.below(5)).min(TEX_SIZE);
                for x in x0..end {
                    img.set(x, y, [18, 16, 16], 0);
                }
            }
            speckle(r, &mut img, 3, &[[100, 92, 70], [30, 26, 24]], 0);
            img
        }
        Tex::CleanPanel => panel(r, [212, 218, 224]),
        Tex::MinerFront | Tex::MinerFrontLit => {
            // The control house's front (rows 8-13 show): a depth display, two lamps, and a
            // stripe along the bottom. Above them (only the icon shows it) the rig's derrick.
            let mut img = panel(r, [96, 102, 112]);
            for y in 1..=7usize {
                let spread = (y - 1) / 2;
                for x in [6 - spread, 9 + spread] {
                    img.set(x, y, [226, 172, 44], 0);
                }
                if y == 4 || y == 7 {
                    for x in 6 - spread..=9 + spread {
                        img.set(x, y, [196, 148, 36], 0);
                    }
                }
            }
            for y in 2..=7usize {
                img.set(7, y, [70, 74, 82], 0);
                img.set(8, y, [52, 56, 64], 0);
            }
            for x in 5..=10usize {
                img.set(x, 0, [70, 74, 82], 0);
            }
            for y in 9..=11usize {
                for x in 4..=8usize {
                    let c = match (lit, y == 10 && x <= 7) {
                        (true, true) => CYAN_HI,
                        (true, false) => CYAN,
                        (false, _) => [24, 30, 40],
                    };
                    img.set(x, y, c, 0);
                }
            }
            for (x, y) in [(10, 9), (11, 9)] {
                img.set(x, y, if lit { [110, 240, 120] } else { [40, 54, 44] }, 0);
            }
            for (x, y) in [(10, 11), (11, 11)] {
                img.set(x, y, if lit { AMBER } else { [60, 46, 30] }, 0);
            }
            for x in 0..TEX_SIZE {
                let c = if (x / 2) % 2 == 0 {
                    [230, 186, 40]
                } else {
                    [30, 30, 32]
                };
                img.set(x, 13, c, 0);
            }
            img
        }
        Tex::PumpFront | Tex::PumpFrontLit => {
            // A dial over a pipe coming up out of the ground.
            let mut img = panel(r, [96, 102, 112]);
            fill(&mut img, r, 6..=9, 9..=15, [30, 30, 32]);
            for y in 2..9usize {
                for x in 3..12usize {
                    let (dx, dy) = (x as f32 - 7.0, y as f32 - 5.0);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 3.2 {
                        img.set(x, y, [226, 226, 216], 0);
                    } else if d < 3.9 {
                        img.set(x, y, [60, 62, 68], 0);
                    }
                }
            }
            let needle = if lit {
                [(8, 4), (9, 3)]
            } else {
                [(6, 5), (5, 5)]
            };
            for (x, y) in needle {
                img.set(x, y, [200, 40, 30], 0);
            }
            lamp(&mut img, lit);
            img
        }
        Tex::RefineryFront | Tex::RefineryFrontLit => {
            // Two pipes with red valve wheels, and a window onto the flame below.
            let mut img = panel(r, [96, 102, 112]);
            for x in [4usize, 10] {
                fill(&mut img, r, x..=x + 1, 1..=14, [150, 152, 158]);
                fill(&mut img, r, x - 1..=x + 2, 5..=6, [190, 50, 40]);
            }
            let flame = if lit { [255, 150, 50] } else { [40, 30, 28] };
            fill(&mut img, r, 6..=9, 10..=13, flame);
            lamp(&mut img, lit);
            img
        }
        Tex::ArcFront | Tex::ArcFrontLit => {
            // A dark window onto three electrodes, blinding between them while it works.
            let mut img = panel(r, [86, 90, 98]);
            fill(&mut img, r, 2..=13, 4..=13, [24, 24, 28]);
            for x in [4usize, 7, 10] {
                fill(&mut img, r, x..=x + 1, 4..=8, [70, 70, 76]);
            }
            if lit {
                for y in 9..=12usize {
                    for x in 3..=12usize {
                        let c = if (x + y) % 3 == 0 {
                            [255, 255, 255]
                        } else {
                            [150, 200, 255]
                        };
                        img.set(x, y, c, 0);
                    }
                }
            }
            lamp(&mut img, lit);
            img
        }
        Tex::AssemblerFront | Tex::AssemblerFrontLit => {
            // A window onto a yellow robot arm over its bench.
            let mut img = panel(r, [96, 102, 112]);
            fill(&mut img, r, 2..=13, 4..=13, [34, 38, 46]);
            fill(&mut img, r, 2..=13, 12..=13, [110, 90, 60]);
            let arm = [
                (4, 11),
                (4, 10),
                (5, 9),
                (6, 8),
                (7, 7),
                (8, 6),
                (9, 6),
                (10, 7),
                (10, 8),
            ];
            for (x, y) in arm {
                img.set(x, y, [236, 186, 40], 0);
                img.set(x + 1, y, [200, 150, 30], 0);
            }
            if lit {
                img.set(10, 9, [255, 240, 160], 0);
                img.set(11, 9, [255, 240, 160], 0);
            }
            lamp(&mut img, lit);
            img
        }
        Tex::FabFront | Tex::FabFrontLit => {
            // A clean white housing, and a blue-lit window with a wafer behind it.
            let mut img = panel(r, [212, 218, 224]);
            let window = if lit { [110, 170, 255] } else { [50, 70, 100] };
            fill(&mut img, r, 3..=12, 4..=12, window);
            for y in 5..12usize {
                for x in 4..12usize {
                    let (dx, dy) = (x as f32 - 7.5, y as f32 - 8.0);
                    if (dx * dx + dy * dy).sqrt() < 3.0 {
                        let c = if lit {
                            [210, 220, 255]
                        } else {
                            [120, 130, 150]
                        };
                        img.set(x, y, c, 0);
                    }
                }
            }
            lamp(&mut img, lit);
            img
        }
        Tex::AccumulatorFront | Tex::AccumulatorFrontLit => {
            // Dark recesses behind the three charge tubes (the tubes are the model's).
            let mut img = panel(r, [96, 102, 112]);
            for x in [3usize, 7, 11] {
                fill(&mut img, r, x - 1..=x + 2, 4..=13, [30, 34, 40]);
            }
            img
        }
        Tex::Lamp => {
            let mut img = noisy(r, [236, 236, 236], 0.02, 255);
            for x in 0..TEX_SIZE {
                img.shade(x, 0, 1.06);
                img.shade(x, 15, 0.9);
            }
            img
        }
        Tex::RigSteel => {
            // Painted steel, worn at the edges.
            let mut img = noisy(r, [220, 164, 40], 0.05, 0);
            for i in 0..TEX_SIZE {
                for edge in [0, 15] {
                    img.shade(i, edge, 0.7);
                    img.shade(edge, i, 0.7);
                }
            }
            speckle(r, &mut img, 6, &[[150, 140, 120], [120, 84, 30]], 0);
            img
        }
        Tex::Hazard => {
            let mut img = noisy(r, [230, 186, 40], 0.04, 0);
            for y in 0..TEX_SIZE {
                for x in 0..TEX_SIZE {
                    if ((x + y) / 4) % 2 == 1 {
                        img.set(x, y, scale([32, 32, 34], 1.0 + r.signed() * 0.1), 0);
                    }
                }
            }
            img
        }
        Tex::CrudeOil => sprite(|x, y| {
            // A barrel with two hoops and an oil drop on it.
            let (xi, yi) = (x as i32, y as i32);
            if !(4..=11).contains(&xi) || !(2..=14).contains(&yi) {
                return None;
            }
            let (dx, dy) = (x - 8.0, y - 9.0);
            let drop = dx * dx + dy * dy < 2.4 || (dx.abs() < 0.6 && (-3.0..-0.5).contains(&dy));
            Some(if yi == 2 {
                [70, 76, 96]
            } else if yi == 5 || yi == 12 {
                [96, 102, 118]
            } else if drop {
                [16, 14, 14]
            } else if xi == 11 {
                [30, 34, 50]
            } else {
                [46, 52, 74]
            })
        }),
        Tex::FuelCan => sprite(|x, y| {
            // A red jerry can: handle and spout on top, an X pressed into its side.
            let (x, y) = (x as i32, y as i32);
            let body = (3..=12).contains(&x) && (5..=14).contains(&y);
            let handle =
                (4..=8).contains(&x) && (2..=4).contains(&y) && !((5..=7).contains(&x) && y == 3);
            let spout = (10..=12).contains(&x) && (2..=4).contains(&y);
            let cross = body && ((x - 3) == (y - 5) || (x - 3) + (y - 5) == 9);
            if spout {
                Some([60, 60, 64])
            } else if handle {
                Some([170, 32, 26])
            } else if cross {
                Some([150, 26, 20])
            } else if body {
                Some(if x == 12 || y == 14 {
                    [160, 30, 24]
                } else {
                    [206, 44, 34]
                })
            } else {
                None
            }
        }),
        Tex::Plastic => sprite(|x, y| {
            // A stack of pale sheets.
            let (x, y) = (x as i32, y as i32);
            let row = (y - 5) / 2;
            if !(5..=12).contains(&y) || !(2 + row..=10 + row).contains(&x) {
                return None;
            }
            Some(if y % 2 == 0 {
                [200, 214, 228]
            } else {
                [236, 240, 246]
            })
        }),
        Tex::Rubber => sprite(|x, y| {
            // A dark block of rubber with soft corners and a sheen.
            let (dx, dy) = ((x - 8.0) / 6.0, (y - 8.5) / 4.5);
            if dx.powi(4) + dy.powi(4) > 1.0 {
                return None;
            }
            Some(if dx + dy < -0.9 {
                [92, 88, 86]
            } else if dx + dy > 0.9 {
                [24, 22, 22]
            } else {
                [46, 43, 42]
            })
        }),
        Tex::Silicon => lump(r, [110, 116, 132], [190, 200, 226], 5.0),
        Tex::Wafer => sprite(|x, y| {
            // A polished disc, its dies in a grid, with a flat edge at the bottom.
            let (dx, dy) = (x - 8.0, y - 8.0);
            if dx * dx + dy * dy > 42.0 || dy > 5.6 {
                return None;
            }
            let grid = (x as i32) % 3 == 0 || (y as i32) % 3 == 0;
            let sheen = ((dx - dy) * 0.25).sin();
            let base = [
                (150.0 + 60.0 * sheen) as u8,
                (160.0 + 40.0 * (sheen * 1.7).cos()) as u8,
                (200.0 + 40.0 * sheen) as u8,
            ];
            Some(if grid { scale(base, 0.8) } else { base })
        }),
        Tex::SteelIngot => ingot([150, 156, 168]),
        Tex::CopperWire => spool(COPPER, [255, 200, 150], COPPER),
        Tex::InsulatedWire => spool([46, 172, 190], [150, 236, 244], COPPER),
        Tex::CircuitBoard => sprite(|x, y| {
            // A deep teal board, cyan traces between pads, a chip, mounting holes.
            let (x, y) = (x as i32, y as i32);
            let edge = [1, 2, 14, 13];
            if !inside(x, y, edge) {
                return None;
            }
            if matches!((x, y), (2, 3) | (13, 3) | (2, 12) | (13, 12)) {
                return Some(STEEL_HI);
            }
            if inside(x, y, [4, 6, 7, 8]) {
                return Some(if (x, y) == (5, 7) {
                    CYAN_HI
                } else {
                    bevel(x, y, [4, 6, 7, 8], SHELL)
                });
            }
            if matches!((x, y), (4, 4) | (12, 7) | (3, 10) | (12, 11) | (10, 4)) {
                return Some(CYAN_HI);
            }
            let trace = (y == 4 && (4..=10).contains(&x))
                || (x == 10 && (4..=7).contains(&y))
                || (y == 7 && (8..=12).contains(&x))
                || (y == 10 && (3..=6).contains(&x))
                || (x == 6 && (9..=10).contains(&y))
                || (x == 12 && (9..=11).contains(&y))
                || (y == 9 && (8..=12).contains(&x));
            Some(if trace {
                CYAN
            } else {
                bevel(x, y, edge, [16, 66, 70])
            })
        }),
        Tex::Battery => sprite(|x, y| {
            // A power cell: a steel cap, rings round a dark shell, and its charge glowing
            // through a window in three bars.
            let (x, y) = (x as i32, y as i32);
            if (6..=9).contains(&x) && (1..=2).contains(&y) {
                return Some(if y == 1 { STEEL_HI } else { STEEL });
            }
            let body = [4, 3, 11, 14];
            let corner = (x == 4 || x == 11) && (y == 3 || y == 14);
            if !inside(x, y, body) || corner {
                return None;
            }
            if y == 4 || y == 13 {
                return Some(if x == 4 { STEEL_HI } else { STEEL });
            }
            if inside(x, y, [6, 5, 9, 12]) {
                let bar = (y - 5) % 3 != 2;
                return Some(match (bar, x) {
                    (false, _) => INK,
                    (true, 6) => [190, 255, 220],
                    (true, _) => MINT,
                });
            }
            Some(bevel(x, y, body, SHELL))
        }),
        Tex::Led => sprite(|x, y| {
            // A glowing bead of light in a steel socket.
            let (dx, dy) = (x - 7.5, y - 6.0);
            let d = (dx * dx + dy * dy).sqrt();
            let (xi, yi) = (x as i32, y as i32);
            if d < 3.7 && yi <= 9 {
                return Some(if d < 1.4 {
                    [255, 255, 255]
                } else if d < 2.5 || (dx < -1.0 && dy < -1.0) {
                    CYAN_HI
                } else {
                    CYAN
                });
            }
            if (4..=11).contains(&xi) && (10..=11).contains(&yi) {
                return Some(if yi == 10 { STEEL_HI } else { STEEL });
            }
            ((xi == 6 || xi == 9) && (12..=14).contains(&yi)).then_some(scale(STEEL, 0.8))
        }),
        Tex::MemoryChip => sprite(|x, y| {
            // A memory crystal: a violet prism holding its bits as points of light, clamped
            // by a band, standing in a socket with its contacts.
            let (x, y) = (x as i32, y as i32);
            if (13..=14).contains(&y) && (3..=12).contains(&x) {
                return Some(if y == 13 {
                    if x == 3 { STEEL_HI } else { STEEL }
                } else if x % 2 == 0 {
                    GOLD
                } else {
                    SHELL
                });
            }
            if y == 7 && (4..=11).contains(&x) {
                return Some(match x {
                    4 => SHELL_HI,
                    11 => CYAN_HI,
                    _ => SHELL,
                });
            }
            let half = match y {
                1 => 1,
                2 => 2,
                3..=12 => 3,
                _ => return None,
            };
            if !(8 - half..=7 + half).contains(&x) {
                return None;
            }
            let bit = (x == 7 || x == 8) && matches!(y, 3 | 5 | 9 | 11) && (x + y) % 2 == 0;
            Some(if bit {
                [255, 255, 255]
            } else if x <= 6 {
                if y <= 2 { VIOLET_HI } else { [196, 142, 255] }
            } else if x >= 9 {
                [104, 52, 196]
            } else {
                VIOLET
            })
        }),
        Tex::Cpu => chip(SHELL, STEEL, |x, y| {
            // A processor: an amber core in square rings, a marker in one corner.
            if (x, y) == (3, 3) {
                return Some(AMBER);
            }
            let d = (x * 2 - 15).abs().max((y * 2 - 15).abs());
            match d {
                0..=1 => Some(AMBER_HI),
                2..=3 => Some(INK),
                4..=5 => Some(AMBER),
                6..=7 => Some(INK),
                _ => None,
            }
        }),
        Tex::AiChip => chip([40, 26, 62], GOLD, |x, y| {
            // A neural chip: a violet diamond with lines out to nodes in the corners.
            let d = ((x * 2 - 15).abs() + (y * 2 - 15).abs()) / 2;
            if matches!((x, y), (3, 3) | (12, 3) | (3, 12) | (12, 12)) {
                return Some(VIOLET_HI);
            }
            match d {
                0 => Some([255, 255, 255]),
                1..=2 => Some(VIOLET_HI),
                3..=4 => Some(VIOLET),
                _ if x == y || x + y == 15 => Some([120, 66, 196]),
                _ => None,
            }
        }),
        Tex::Monitor => sprite(|x, y| {
            // A screen: a thin dark frame round a glowing display, its edge showing below
            // and to the right.
            let (x, y) = (x as i32, y as i32);
            if (y == 12 && (2..=14).contains(&x)) || (x == 14 && (4..=12).contains(&y)) {
                return Some(SHELL);
            }
            if !inside(x, y, [1, 3, 13, 11]) {
                return None;
            }
            if !inside(x, y, [2, 4, 12, 10]) {
                return Some(if y == 3 || x == 1 { SHELL_HI } else { INK });
            }
            let t = (y - 4) as f32 / 6.0;
            let glare = (3..=4).contains(&(x - y + 2));
            let c = if glare {
                [170, 226, 255]
            } else {
                [
                    (70.0 - 40.0 * t) as u8,
                    (186.0 - 90.0 * t) as u8,
                    (246.0 - 70.0 * t) as u8,
                ]
            };
            Some(if y % 2 == 1 { scale(c, 0.9) } else { c })
        }),
        Tex::Motherboard => sprite(|x, y| {
            // A mainboard: navy, cyan traces running into a steel socket with an amber core,
            // two slots down its side.
            let (x, y) = (x as i32, y as i32);
            let edge = [1, 1, 14, 14];
            if !inside(x, y, edge) {
                return None;
            }
            if inside(x, y, [4, 4, 9, 9]) {
                return Some(if inside(x, y, [5, 5, 8, 8]) {
                    if inside(x, y, [6, 6, 7, 7]) {
                        AMBER_HI
                    } else {
                        AMBER
                    }
                } else {
                    bevel(x, y, [4, 4, 9, 9], STEEL)
                });
            }
            if (x == 11 || x == 13) && (4..=12).contains(&y) {
                return Some(if y == 4 { CYAN_HI } else { INK });
            }
            if matches!((x, y), (3, 12) | (6, 12) | (9, 12)) {
                return Some(CYAN_HI);
            }
            let trace = (y == 2 && (3..=12).contains(&x))
                || (x == 3 && (2..=12).contains(&y))
                || (y == 12 && (3..=9).contains(&x))
                || (x == 6 && (10..=12).contains(&y))
                || (x == 9 && (10..=12).contains(&y))
                || (x == 12 && (2..=3).contains(&y));
            Some(if trace {
                scale(CYAN, 0.8)
            } else {
                bevel(x, y, edge, [26, 32, 58])
            })
        }),
        Tex::PowerSupply => sprite(|x, y| {
            // A power unit: a heavy shell with a glowing coil behind a round window, a vent,
            // and two terminals on top.
            let (xi, yi) = (x as i32, y as i32);
            if (xi == 5 || xi == 10) && (1..=2).contains(&yi) {
                return Some(if yi == 1 { STEEL_HI } else { STEEL });
            }
            let body = [2, 3, 13, 13];
            if !inside(xi, yi, body) {
                return None;
            }
            let (dx, dy) = (x - 6.5, y - 8.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 3.9 {
                return Some(if d < 1.3 {
                    AMBER_HI
                } else if d < 2.2 {
                    AMBER
                } else if d < 3.1 {
                    if (xi + yi) % 2 == 0 {
                        COPPER
                    } else {
                        [150, 74, 40]
                    }
                } else {
                    STEEL
                });
            }
            if (11..=12).contains(&xi) && (5..=11).contains(&yi) {
                return Some(if yi % 2 == 0 { INK } else { SHELL_HI });
            }
            Some(bevel(xi, yi, body, SHELL))
        }),
        Tex::ComputerCase => sprite(|x, y| {
            // An empty server chassis, seen from above its front: the top, the side, rack
            // ears with their holes, and the bays waiting for what goes in them.
            let (x, y) = (x as i32, y as i32);
            if (y == 4 && (3..=14).contains(&x)) || (y == 5 && (2..=13).contains(&x)) {
                return Some(if y == 4 {
                    scale(SHELL_HI, 1.2)
                } else {
                    SHELL_HI
                });
            }
            if (x == 14 && (5..=10).contains(&y)) || (x == 13 && y == 11) {
                return Some([30, 34, 46]);
            }
            let front = [1, 6, 13, 11];
            if !inside(x, y, front) {
                return None;
            }
            if x == 1 || x == 13 {
                return Some(if y == 8 { INK } else { STEEL });
            }
            if inside(x, y, [3, 7, 11, 8]) || inside(x, y, [3, 10, 11, 10]) {
                return Some(INK);
            }
            Some(bevel(x, y, front, SHELL))
        }),
        Tex::Computer => sprite(|x, y| {
            // A server: three units stacked, each with its lights and an activity bar, vents,
            // a light strip down the side.
            let (x, y) = (x as i32, y as i32);
            if (y == 1 && (4..=14).contains(&x)) || (y == 2 && (3..=13).contains(&x)) {
                return Some(if y == 1 {
                    scale(SHELL_HI, 1.2)
                } else {
                    SHELL_HI
                });
            }
            if x == 14 && (2..=13).contains(&y) {
                return Some([28, 32, 44]);
            }
            if !inside(x, y, [2, 3, 13, 14]) {
                return None;
            }
            if x == 2 {
                return Some(if (6..=10).contains(&y) { CYAN_HI } else { CYAN });
            }
            if y == 14 {
                return Some(INK);
            }
            let (unit, row) = ((y - 3) / 4, (y - 3) % 4);
            if row == 0 {
                return Some(SHELL_HI);
            }
            let lights = [MINT, AMBER, MINT];
            Some(match (row, x) {
                (1, 4) => CYAN_HI,
                (1, 5) => lights[unit as usize % 3],
                (2, 4..=7) => {
                    if x - 4 <= 1 + unit {
                        CYAN
                    } else {
                        INK
                    }
                }
                (1..=2, 9..=12) if (x + y) % 2 == 0 => INK,
                (3, _) => scale(SHELL, 0.7),
                _ => SHELL,
            })
        }),
        _ => noisy(r, [255, 0, 255], 0.0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes what factories make, eight times their size on a grey ground, to
    /// `target/industry_sprites.bmp` (to look at them).
    #[test]
    #[ignore]
    fn write_industry_sprites() {
        const ZOOM: usize = 8;
        let sprites = &Tex::ALL[Tex::CrudeOil as usize..];
        let columns = 7;
        let rows = sprites.len().div_ceil(columns);
        let cell = TEX_SIZE * ZOOM + 8;
        let (w, h) = (columns * cell, rows * cell);
        let mut rgb = vec![[96u8, 102, 116]; w * h];
        for (i, &t) in sprites.iter().enumerate() {
            let px = super::super::pixels(t);
            let (ox, oy) = ((i % columns) * cell + 4, (i / columns) * cell + 4);
            for y in 0..TEX_SIZE * ZOOM {
                for x in 0..TEX_SIZE * ZOOM {
                    let p = px[(y / ZOOM) * TEX_SIZE + x / ZOOM];
                    if p[3] >= 128 {
                        rgb[(oy + y) * w + ox + x] = [p[0], p[1], p[2]];
                    }
                }
            }
        }
        let row = (w * 3).div_ceil(4) * 4;
        let mut file = Vec::new();
        let size = 54 + row * h;
        file.extend(b"BM");
        file.extend((size as u32).to_le_bytes());
        file.extend([0u8; 4]);
        file.extend(54u32.to_le_bytes());
        file.extend(40u32.to_le_bytes());
        file.extend((w as i32).to_le_bytes());
        file.extend((h as i32).to_le_bytes());
        file.extend(1u16.to_le_bytes());
        file.extend(24u16.to_le_bytes());
        file.extend([0u8; 24]);
        for y in (0..h).rev() {
            let start = file.len();
            for x in 0..w {
                let [r, g, b] = rgb[y * w + x];
                file.extend([b, g, r]);
            }
            file.resize(start + row, 0);
        }
        std::fs::create_dir_all("target").unwrap();
        std::fs::write("target/industry_sprites.bmp", file).unwrap();
    }
}
