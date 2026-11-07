//! Cube-sphere planet geometry.
//!
//! A planet is a solid ball of blocks. Its 6 cube faces are each a grid of block *columns*,
//! inflated onto a sphere; each column is a stack of *layers* going radially outward from the
//! centre. A block is addressed as `(face, u, v, k)`: face, two grid coordinates, layer.
//!
//! * Blocks are slightly wedge-shaped but look and behave like cubes, and "up" is always
//!   radial - so building works the same everywhere on the planet.
//! * The equiangular (tangent) mapping keeps blocks close to the same width over a whole
//!   face, instead of the naive normalized-cube mapping which squashes them near corners.
//! * At face edges, neighbours live on another face with a rotated grid;
//!   [`Planet::wrap_column`] handles that.
//!
//! # Shells
//! Columns converge towards the centre, so a fixed grid would make deep blocks infinitely
//! thin. Instead the planet is made of nested **shells**: whenever blocks have shrunk to half
//! their surface width, the grid resolution halves (each 2x2 group of columns becomes one
//! column below the shell boundary). Block widths therefore stay within ~0.5-1 m all the way
//! down; only the small innermost shell's blocks taper towards the very centre. Shell
//! boundaries are aligned to chunk layers, so every chunk lies in exactly one shell.
//! Layer 0 starts at the centre of the planet.

use glam::DVec3;

use super::chunk::{BlockCell, ChunkKey};

pub const CHUNK_SIZE: i32 = 32;

/// The cube-to-sphere mapping: a grid coordinate x in [-1, 1] across a face becomes the cube
/// coordinate tan(MAP_ANGLE * x) / tan(MAP_ANGLE).
///
/// No grid of blocks can cover a sphere with equal cubes. 45 degrees would be the classic
/// equiangular map: perfect cubes at face centres, but 29% narrower along face edges. 38
/// degrees, together with the radius chosen in [`Planet::new`], spreads the width error
/// evenly instead. Surface blocks are 0.84-1.19 m wide everywhere, 9% off on average, and 61%
/// of them are within 10% of 1 m (equiangular: 0.71-1.0 m, 12%, 44%).
///
/// Away from the face's centre lines blocks are also skewed (parallelograms, 9 degrees on
/// average), up to 120-degree diamonds at the 8 cube corners. That comes from the grid lines
/// being great circles, so no mapping of this kind can change it.
pub const MAP_ANGLE: f64 = 0.663_225_115_757_845_3; // 38 degrees
const TAN_MAP_ANGLE: f64 = 0.781_285_626_506_717_4;

/// One of the 6 cube faces: 0 = +X, 1 = -X, 2 = +Y, 3 = -Y, 4 = +Z, 5 = -Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Face(pub u8);

impl Face {
    pub const ALL: [Face; 6] = [Face(0), Face(1), Face(2), Face(3), Face(4), Face(5)];

    /// `(normal, s, t)` with `s x t = normal`, so (u, v, k) is a right-handed system.
    pub fn basis(self) -> (DVec3, DVec3, DVec3) {
        let axes = [DVec3::X, DVec3::Y, DVec3::Z];
        let axis = (self.0 / 2) as usize;
        let (a, b) = (axes[(axis + 1) % 3], axes[(axis + 2) % 3]);
        if self.0.is_multiple_of(2) {
            (axes[axis], a, b)
        } else {
            (-axes[axis], b, a)
        }
    }

    /// The face whose normal is closest to `dir`.
    pub fn from_direction(dir: DVec3) -> Face {
        let a = dir.abs();
        let axis = if a.x >= a.y && a.x >= a.z {
            0
        } else if a.y >= a.z {
            1
        } else {
            2
        };
        Face(axis as u8 * 2 + if dir[axis] > 0.0 { 0 } else { 1 })
    }
}

/// A radial range of layers sharing one grid resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shell {
    /// First layer of the shell (a multiple of [`CHUNK_SIZE`]).
    pub bottom: i32,
    /// Columns along each face edge.
    pub res: i32,
}

#[derive(Clone, Debug)]
pub struct Planet {
    /// Absolute world position of the planet's centre.
    pub center: DVec3,
    /// Columns along each face edge at the surface (`N`).
    pub face_blocks: i32,
    /// Total number of block layers, from the centre up to the build limit.
    pub layers: i32,
    /// Layer index of the sea surface.
    pub sea_layer: i32,
    /// Radius of the sea surface in meters.
    pub sea_radius: f64,
    /// Shells from the surface (index 0) inwards; the last one reaches the centre.
    pub shells: Vec<Shell>,
    pub seed: u64,
}

impl Planet {
    /// `chunks_per_face` sets the size: the sea-level radius is chosen so surface blocks are
    /// ~1 m wide (0.84-1.19 m, see [`MAP_ANGLE`]). `height_limit` is how many layers exist
    /// above sea level.
    pub fn new(center: DVec3, chunks_per_face: i32, height_limit: i32, seed: u64) -> Self {
        let face_blocks = chunks_per_face * CHUNK_SIZE;
        // Blocks at a face centre are R * (2 / N) * MAP_ANGLE / tan(MAP_ANGLE) wide, the
        // widest anywhere. Blocks on a face edge are 1/sqrt(2) of that along the edge, the
        // narrowest. Balance the two around 1 m: centre 2^(1/4) = 1.19 m, edge 0.84 m.
        let sea_radius = face_blocks as f64 * 2f64.powf(0.25) * TAN_MAP_ANGLE / (2.0 * MAP_ANGLE);
        let sea_layer = sea_radius.floor() as i32;
        let layers = (sea_layer + height_limit + CHUNK_SIZE - 1) / CHUNK_SIZE * CHUNK_SIZE;

        let mut planet = Self {
            center,
            face_blocks,
            layers,
            sea_layer,
            sea_radius,
            shells: Vec::new(),
            seed,
        };

        // Halve the grid each time blocks shrink to half their surface width.
        let mut res = face_blocks;
        let mut boundary_radius = sea_radius / 2.0;
        loop {
            let bottom = (planet.layer_at_radius(boundary_radius) / CHUNK_SIZE as f64).round()
                as i32
                * CHUNK_SIZE;
            if res / 2 < 2 * CHUNK_SIZE || bottom < 2 * CHUNK_SIZE {
                planet.shells.push(Shell { bottom: 0, res });
                break;
            }
            planet.shells.push(Shell { bottom, res });
            res /= 2;
            boundary_radius /= 2.0;
        }
        planet
    }

    pub fn chunk_layers(&self) -> i32 {
        self.layers / CHUNK_SIZE
    }

    /// Index of the shell containing layer `k`.
    pub fn shell_index(&self, k: i32) -> usize {
        self.shells
            .iter()
            .position(|s| k >= s.bottom)
            .unwrap_or(self.shells.len() - 1)
    }

    /// Grid resolution (columns per face edge) at layer `k`.
    pub fn res_at(&self, k: i32) -> i32 {
        self.shells[self.shell_index(k)].res
    }

    /// Radius (m) of the bottom of layer `k` (fractional `k` allowed).
    pub fn layer_radius(&self, k: f64) -> f64 {
        self.sea_radius + (k - self.sea_layer as f64)
    }

    /// Inverse of [`layer_radius`](Self::layer_radius).
    pub fn layer_at_radius(&self, r: f64) -> f64 {
        r - self.sea_radius + self.sea_layer as f64
    }

    /// Grid coordinate (0..=n, fractional allowed) on a face with `n` columns per edge ->
    /// cube-face coordinate in [-1, 1]. Computed symmetrically so mirrored coordinates give
    /// bit-identical results, which keeps vertices on shared face edges exactly equal.
    pub fn grid_to_cube(g: f64, n: i32) -> f64 {
        let n = n as f64;
        let x = (2.0 * g - n) / n;
        if x.abs() == 1.0 {
            return x;
        }
        ((x.abs() * MAP_ANGLE).tan() / TAN_MAP_ANGLE).copysign(x)
    }

    fn grid_to_cube_exact(g: i32, n: i32) -> f64 {
        if g == 0 {
            -1.0
        } else if g == n {
            1.0
        } else if 2 * g == n {
            0.0
        } else {
            Self::grid_to_cube(g as f64, n)
        }
    }

    /// Inverse of [`grid_to_cube`](Self::grid_to_cube).
    pub fn cube_to_grid(a: f64, n: i32) -> f64 {
        ((a * TAN_MAP_ANGLE).atan() / MAP_ANGLE + 1.0) * 0.5 * n as f64
    }

    /// Footprint of the surface block in direction `dir`: its widths along u and v (m) and
    /// the angle between its edges (degrees). Blocks are 1 m tall everywhere.
    pub fn block_shape(&self, dir: DVec3) -> (f64, f64, f64) {
        let (face, n) = (Face::from_direction(dir), self.face_blocks);
        let (u, v, _) = self.locate_on_face(face, dir * self.sea_radius, n);
        let (u, v) = (
            (u.floor() as i32).clamp(0, n - 1),
            (v.floor() as i32).clamp(0, n - 1),
        );
        let corner = |du, dv| self.corner_dir(face, u + du, v + dv, n) * self.sea_radius;
        let (eu, ev) = (corner(1, 0) - corner(0, 0), corner(0, 1) - corner(0, 0));
        let angle = eu.normalize().dot(ev.normalize()).acos().to_degrees();
        (eu.length(), ev.length(), angle)
    }

    /// Unit direction through grid position (u, v) on a face with `n` columns per edge.
    pub fn dir(&self, face: Face, u: f64, v: f64, n: i32) -> DVec3 {
        let (normal, s, t) = face.basis();
        (normal + s * Self::grid_to_cube(u, n) + t * Self::grid_to_cube(v, n)).normalize()
    }

    /// Unit direction through an integer grid corner. Exact on face edges, so neighbouring
    /// faces produce identical vertices (no cracks). A coarse shell's corners also coincide
    /// exactly with every other corner of the finer shell above it.
    pub fn corner_dir(&self, face: Face, u: i32, v: i32, n: i32) -> DVec3 {
        let (normal, s, t) = face.basis();
        (normal + s * Self::grid_to_cube_exact(u, n) + t * Self::grid_to_cube_exact(v, n))
            .normalize()
    }

    /// Direction through the centre of surface column (u, v).
    pub fn column_dir(&self, face: Face, u: i32, v: i32) -> DVec3 {
        self.dir(face, u as f64 + 0.5, v as f64 + 0.5, self.face_blocks)
    }

    /// Planet-relative position -> (face, u, v, k), all fractional. `u` and `v` are in grid
    /// units of the shell that layer `k` belongs to.
    pub fn locate(&self, rel: DVec3) -> (Face, f64, f64, f64) {
        let face = Face::from_direction(rel);
        let (normal, s, t) = face.basis();
        let k = self.layer_at_radius(rel.length());
        let n = self.res_at((k.floor() as i32).clamp(0, self.layers - 1));
        let dn = rel.dot(normal).max(1e-12);
        let u = Self::cube_to_grid(rel.dot(s) / dn, n);
        let v = Self::cube_to_grid(rel.dot(t) / dn, n);
        (face, u, v, k)
    }

    /// Like [`locate`](Self::locate), but in the grid of a given face and resolution `n`
    /// (coordinates may run outside 0..n near the face's edges). Used to keep one consistent
    /// grid while something crosses a face edge.
    pub fn locate_on_face(&self, face: Face, rel: DVec3, n: i32) -> (f64, f64, f64) {
        let (normal, s, t) = face.basis();
        let k = self.layer_at_radius(rel.length());
        let dn = rel.dot(normal).max(1e-12);
        let u = Self::cube_to_grid(rel.dot(s) / dn, n);
        let v = Self::cube_to_grid(rel.dot(t) / dn, n);
        (u, v, k)
    }

    /// The cell at grid position (u, v) of `face` at layer `k`, with u and v in the grid of
    /// layer k's shell and possibly beyond the face's edge (wrapped onto the right face).
    /// `None` outside the planet's layers.
    pub fn cell(&self, face: Face, u: i32, v: i32, k: i32) -> Option<BlockCell> {
        if !(0..self.layers).contains(&k) {
            return None;
        }
        let n = self.res_at(k);
        let (face, u, v) = self.wrap_column(face, u, v, n);
        Some(BlockCell { face, u, v, k })
    }

    /// The cell containing a planet-relative position.
    pub fn cell_at(&self, rel: DVec3) -> Option<BlockCell> {
        let (face, u, v, k) = self.locate(rel);
        self.cell(face, u.floor() as i32, v.floor() as i32, k.floor() as i32)
    }

    /// The chunk holding a cell, and the cell's chunk-local coordinates.
    pub fn cell_chunk(&self, cell: BlockCell) -> (ChunkKey, [i32; 3]) {
        let key = ChunkKey {
            face: cell.face,
            cu: cell.u.div_euclid(CHUNK_SIZE),
            cv: cell.v.div_euclid(CHUNK_SIZE),
            ck: cell.k.div_euclid(CHUNK_SIZE),
        };
        let local = [
            cell.u.rem_euclid(CHUNK_SIZE),
            cell.v.rem_euclid(CHUNK_SIZE),
            cell.k.rem_euclid(CHUNK_SIZE),
        ];
        (key, local)
    }

    /// The 8 corners of a cell (planet-relative), indexed by `du + 2 dv + 4 dk`.
    pub fn cell_corners(&self, cell: BlockCell) -> [DVec3; 8] {
        let n = self.res_at(cell.k);
        std::array::from_fn(|i| {
            let (du, dv, dk) = ((i & 1) as i32, ((i >> 1) & 1) as i32, (i >> 2) as i32);
            self.corner_dir(cell.face, cell.u + du, cell.v + dv, n)
                * self.layer_radius((cell.k + dk) as f64)
        })
    }

    /// The chunk containing a planet-relative position (clamped to the valid layer range).
    pub fn chunk_at(&self, rel: DVec3) -> ChunkKey {
        let (face, u, v, k) = self.locate(rel);
        let ck = (k.floor() as i32).clamp(0, self.layers - 1) / CHUNK_SIZE;
        let n = self.res_at(ck * CHUNK_SIZE);
        let clamp = |x: f64| (x.floor() as i32).clamp(0, n - 1) / CHUNK_SIZE;
        ChunkKey {
            face,
            cu: clamp(u),
            cv: clamp(v),
            ck,
        }
    }

    /// Maps a column coordinate that may lie outside its face (by any amount along *one*
    /// axis) onto the face it really belongs to, for a grid of `n` columns per face edge.
    /// Coordinates outside along both axes (the cube corners, where only 3 faces meet) are
    /// approximated.
    pub fn wrap_column(&self, face: Face, u: i32, v: i32, n: i32) -> (Face, i32, i32) {
        let in_u = (0..n).contains(&u);
        let in_v = (0..n).contains(&v);
        if in_u && in_v {
            return (face, u, v);
        }
        let v = if !in_u && !in_v { v.clamp(0, n - 1) } else { v };

        let (normal, s, t) = face.basis();
        // Which edge we crossed, how far, and the coordinate shared with the next face.
        let (axis, sign, depth, shared_axis, shared) = if !in_u {
            let (sign, depth) = if u < 0 { (-1.0, -u) } else { (1.0, u - n + 1) };
            (s, sign, depth, t, v)
        } else {
            let (sign, depth) = if v < 0 { (-1.0, -v) } else { (1.0, v - n + 1) };
            (t, sign, depth, s, u)
        };
        let new_normal = axis * sign;
        let new_face = Face::from_direction(new_normal);
        let (_, s2, t2) = new_face.basis();

        // A point on the shared edge, at the centre of the shared column coordinate.
        let p = normal + new_normal + shared_axis * Self::grid_to_cube(shared as f64 + 0.5, n);
        let edge_cell = |c: f64| if c > 0.0 { n - depth } else { depth - 1 };
        let shared_cell = |c: f64| (Self::cube_to_grid(c, n).floor() as i32).clamp(0, n - 1);

        if s2.dot(normal).abs() > 0.5 {
            (new_face, edge_cell(p.dot(s2)), shared_cell(p.dot(t2)))
        } else {
            (new_face, shared_cell(p.dot(s2)), edge_cell(p.dot(t2)))
        }
    }

    /// Centre of a chunk, relative to the planet centre.
    pub fn chunk_center(&self, key: ChunkKey) -> DVec3 {
        let half = (CHUNK_SIZE / 2) as f64;
        let n = self.res_at(key.ck * CHUNK_SIZE);
        let u = (key.cu * CHUNK_SIZE) as f64 + half;
        let v = (key.cv * CHUNK_SIZE) as f64 + half;
        let k = (key.ck * CHUNK_SIZE) as f64 + half;
        self.dir(key.face, u, v, n) * self.layer_radius(k)
    }

    /// All chunks sharing a face with `key`, appended to `out`: 4 sideways (wrapping across
    /// cube faces), and radially 1 chunk - or 4 when crossing up into a finer shell.
    pub fn chunk_neighbors(&self, key: ChunkKey, out: &mut Vec<ChunkKey>) {
        let k0 = key.ck * CHUNK_SIZE;
        let n = self.res_at(k0);
        for (du, dv) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let pick = |delta: i32, base: i32| match delta {
                1 => base + CHUNK_SIZE,
                -1 => base - 1,
                _ => base + CHUNK_SIZE / 2,
            };
            let u = pick(du, key.cu * CHUNK_SIZE);
            let v = pick(dv, key.cv * CHUNK_SIZE);
            let (face, u, v) = self.wrap_column(key.face, u, v, n);
            out.push(ChunkKey {
                face,
                cu: u / CHUNK_SIZE,
                cv: v / CHUNK_SIZE,
                ck: key.ck,
            });
        }

        // Up: same grid, or 2x2 chunks of the finer shell above.
        if key.ck + 1 < self.chunk_layers() {
            let ck = key.ck + 1;
            if self.res_at(ck * CHUNK_SIZE) == n {
                out.push(ChunkKey { ck, ..key });
            } else {
                for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    out.push(ChunkKey {
                        cu: key.cu * 2 + a,
                        cv: key.cv * 2 + b,
                        ck,
                        ..key
                    });
                }
            }
        }
        // Down: same grid, or the coarser chunk below.
        if key.ck > 0 {
            let ck = key.ck - 1;
            if self.res_at(ck * CHUNK_SIZE) == n {
                out.push(ChunkKey { ck, ..key });
            } else {
                out.push(ChunkKey {
                    cu: key.cu / 2,
                    cv: key.cv / 2,
                    ck,
                    ..key
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn planet() -> Planet {
        Planet::new(DVec3::ZERO, 64, 128, 1)
    }

    #[test]
    fn mapping_constants() {
        assert_eq!(MAP_ANGLE, 38f64.to_radians());
        assert!((TAN_MAP_ANGLE - MAP_ANGLE.tan()).abs() < 1e-15);
        for n in [64, 4096] {
            for g in [
                0.0,
                0.3,
                1.0,
                17.5,
                n as f64 / 2.0,
                n as f64 - 0.25,
                n as f64,
            ] {
                let back = Planet::cube_to_grid(Planet::grid_to_cube(g, n), n);
                assert!((back - g).abs() < 1e-9, "{g} -> {back}");
            }
            assert_eq!(Planet::grid_to_cube(n as f64, n), 1.0);
            assert_eq!(Planet::grid_to_cube(0.0, n), -1.0);
        }
    }

    /// Surface blocks stay within 0.84-1.19 m wide everywhere and are mostly close to 1 m.
    #[test]
    fn surface_blocks_are_nearly_cubes() {
        let p = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let (mut lo, mut hi, mut close, mut total) = (f64::MAX, 0f64, 0, 0);
        let (mut close_widths, mut deviation, mut skew) = (0, 0.0, 0.0);
        let steps = 120;
        for i in 0..steps {
            for j in 0..steps {
                // Evenly spread blocks of one face (all faces are the same).
                let n = p.face_blocks as f64;
                let (u, v) = (
                    (i as f64 + 0.5) / steps as f64 * n,
                    (j as f64 + 0.5) / steps as f64 * n,
                );
                let dir = p.dir(Face(0), u, v, p.face_blocks);
                let (wu, wv, angle) = p.block_shape(dir);
                lo = lo.min(wu).min(wv);
                hi = hi.max(wu).max(wv);
                total += 1;
                let worst = (wu - 1.0).abs().max((wv - 1.0).abs());
                deviation += worst;
                skew += (angle - 90.0).abs();
                if worst < 0.1 {
                    close_widths += 1;
                    if (angle - 90.0).abs() < 10.0 {
                        close += 1;
                    }
                }
            }
        }
        let share = close as f64 / total as f64;
        println!(
            "widths {lo:.3}..{hi:.3} m, mean worst-axis deviation {:.1}%, mean skew {:.1} deg, \
             {:.0}% within 10% of 1 m ({:.0}% ignoring skew)",
            deviation / total as f64 * 100.0,
            skew / total as f64,
            share * 100.0,
            close_widths as f64 / total as f64 * 100.0
        );
        // The equiangular map (45 degrees) was 0.71..1.0 m, 12% mean deviation, 44% within 10%.
        assert!(lo > 0.84 && hi < 1.19, "widths {lo}..{hi}");
        assert!(deviation / (total as f64) < 0.095);
        assert!(close_widths as f64 / total as f64 > 0.55);
        // Skew comes from the grid lines being great circles; no warp of the grid changes it.
        assert!(skew / (total as f64) < 9.0);
        assert!(share > 0.3);
    }

    #[test]
    fn shells_reach_the_centre_with_sane_block_sizes() {
        for chunks_per_face in [64, 2048] {
            let p = Planet::new(DVec3::ZERO, chunks_per_face, 1024, 1);
            assert_eq!(p.shells[0].res, p.face_blocks);
            assert_eq!(p.shells.last().unwrap().bottom, 0);
            for pair in p.shells.windows(2) {
                assert!(pair[0].bottom > pair[1].bottom);
                assert_eq!(pair[0].res, pair[1].res * 2);
                assert_eq!(pair[0].bottom % CHUNK_SIZE, 0);
            }
            // Average block width at the top and bottom of every shell except the innermost.
            for (i, s) in p.shells.iter().enumerate().take(p.shells.len() - 1) {
                let top = if i == 0 {
                    p.sea_layer
                } else {
                    p.shells[i - 1].bottom
                };
                let width = |k: i32| p.layer_radius(k as f64) * PI / 2.0 / s.res as f64;
                assert!(width(top) < 1.25, "shell {i} top width {}", width(top));
                assert!(
                    width(s.bottom) > 0.4,
                    "shell {i} bottom width {}",
                    width(s.bottom)
                );
            }
        }
    }

    #[test]
    fn neighbours_are_symmetric_across_shells() {
        let p = planet();
        let mut out = Vec::new();
        for (i, s) in p.shells.iter().enumerate().skip(1) {
            let _ = i;
            // The coarse chunk right below a shell boundary and its fine neighbours above.
            let boundary_ck = p.shells[p.shell_index(s.bottom) - 1].bottom / CHUNK_SIZE;
            let coarse = ChunkKey {
                face: Face(2),
                cu: 1,
                cv: 0,
                ck: boundary_ck - 1,
            };
            out.clear();
            p.chunk_neighbors(coarse, &mut out);
            let fine: Vec<_> = out
                .iter()
                .copied()
                .filter(|k| k.ck == boundary_ck)
                .collect();
            assert_eq!(fine.len(), 4);
            for f in fine {
                let mut back = Vec::new();
                p.chunk_neighbors(f, &mut back);
                assert!(
                    back.contains(&coarse),
                    "{f:?} doesn't link back to {coarse:?}"
                );
            }
        }
    }

    #[test]
    fn wrap_round_trips_across_every_edge() {
        let p = planet();
        let n = p.face_blocks;
        for face in Face::ALL {
            for i in [0, 1, n / 3, n / 2, n - 2, n - 1] {
                for (u, v, back) in [
                    (-1, i, (0, i)),
                    (n, i, (n - 1, i)),
                    (i, -1, (i, 0)),
                    (i, n, (i, n - 1)),
                ] {
                    let (f2, a, b) = p.wrap_column(face, u, v, n);
                    assert_ne!(f2, face);
                    // Step back towards the original face (along the new face's tangent
                    // axis that points along the original face's normal).
                    let (normal, _, _) = face.basis();
                    let (_, s2, t2) = f2.basis();
                    let (bu, bv) = match (s2.dot(normal), t2.dot(normal)) {
                        (x, _) if x > 0.5 => (n, b),
                        (x, _) if x < -0.5 => (-1, b),
                        (_, y) if y > 0.5 => (a, n),
                        _ => (a, -1),
                    };
                    let (f3, x, y) = p.wrap_column(f2, bu, bv, n);
                    assert_eq!(
                        (f3, x, y),
                        (face, back.0, back.1),
                        "face {face:?} ({u},{v})"
                    );
                }
            }
        }
    }

    #[test]
    fn locate_inverts_dir_in_every_shell() {
        let p = planet();
        for s in &p.shells {
            let k = s.bottom as f64 + 3.5;
            let (gu, gv) = (s.res as f64 * 0.13, s.res as f64 * 0.71);
            for face in Face::ALL {
                let dir = p.dir(face, gu, gv, s.res);
                let (f, u, v, kk) = p.locate(dir * p.layer_radius(k));
                assert_eq!(f, face);
                assert!((u - gu).abs() < 1e-6 && (v - gv).abs() < 1e-6);
                assert!((kk - k).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn shared_edges_are_bit_identical() {
        let p = planet();
        let n = p.face_blocks;
        for face in Face::ALL {
            for i in 0..=n {
                let d = p.corner_dir(face, 0, i, n);
                // The same 3D point must be produced by the neighbouring face's grid.
                let (f2, _, _) = p.wrap_column(face, -1, i.min(n - 1), n);
                let (n2, s2, t2) = f2.basis();
                let dn = d.dot(n2);
                let gu = Planet::cube_to_grid(d.dot(s2) / dn, n);
                let gv = Planet::cube_to_grid(d.dot(t2) / dn, n);
                let d2 = p.corner_dir(f2, gu.round() as i32, gv.round() as i32, n);
                assert_eq!(d, d2);
            }
        }
    }

    #[test]
    fn coarse_corners_match_fine_corners() {
        let p = planet();
        let (fine, coarse) = (p.shells[0].res, p.shells[1].res);
        for face in Face::ALL {
            for i in 0..=coarse {
                assert_eq!(
                    p.corner_dir(face, i, 7, coarse),
                    p.corner_dir(face, 2 * i, 14, fine)
                );
            }
        }
    }
}
