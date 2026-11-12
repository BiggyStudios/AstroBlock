//! Cube-sphere planet geometry.
//!
//! A planet is a solid ball of blocks. Its 6 cube faces are each a grid of block *columns*,
//! inflated onto a sphere; each column is a stack of *layers* going radially outward from the
//! centre. A block is addressed as `(face, u, v, k)`: face, two grid coordinates, layer.
//!
//! * Blocks are slightly wedge-shaped but look and behave like cubes, and "up" is always
//!   radial - so building works the same everywhere on the planet.
//! * Each face's grid is mapped onto the sphere by a fitted, nearly *conformal* map (see
//!   [`MAP_Q`]): blocks stay square almost everywhere, and the distortion no grid can avoid
//!   gathers round the 8 cube corners (which terrain generation covers with sea).
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

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4};

use glam::DVec3;

use super::chunk::{BlockCell, ChunkKey};

pub const CHUNK_SIZE: i32 = 32;

/// The cube-to-sphere mapping of a face: grid coordinates (x, y) in [-1, 1]^2 across the face
/// become cube-face coordinates (a, b) = (A(x, y), A(y, x)), and the direction is
/// normalize(normal + a s + b t), with
///
///     A(x, y) = x (1 + (1 - x^2) Q(x^2, y^2)),   Q = sum of MAP_Q[(i, j)] X^i Y^j.
///
/// No grid of blocks can cover a sphere with equal squares. At the 8 cube corners only 3
/// blocks meet, so the grid has to bend somewhere. A separable map (each axis on its own, like
/// the earlier tan(38 deg x) / tan(38 deg)) makes every grid line a great circle. That skews
/// blocks into parallelograms over much of the planet: only 28% were within 3 degrees of
/// square, and 9% were 20-30 degrees off.
///
/// Q was fitted (least squares over the face) to make the map close to *conformal*, i.e.
/// angle-preserving, so blocks are squares. Skew and aspect ratio are ignored only within
/// 0.3 face half-widths of a corner, and the spread of sizes is kept small. The distortion
/// therefore gathers round the corners, under the corner seas of `terrain`:
/// - 92% of surface blocks are within 1 degree of square (96% within 3).
/// - Widths are within 10% of the median ([`BLOCK_WIDTH`]) for 80% of blocks and within 20% for
///   91%. They run from 1.08x the median at face centres down to about 0.5x right at a corner. A square grid on a sphere
///   must change size somewhere, and this map does it towards the corners.
///
/// The form keeps every face edge on the cube's edge (A(+-1, y) = +-1 exactly). A is odd in x
/// and even in y, evaluated from |x| and y^2, so mirrored grid coordinates give bit-identical
/// results. That is what keeps shared face edges and shell corners exact.
const MAP_Q: [(usize, usize, f64); 15] = [
    (0, 0, -1.918_645_132_814_316e-1),
    (0, 1, 1.394_210_225_478_188e-1),
    (1, 0, -2.126_439_194_195_621e-2),
    (0, 2, 1.130_755_627_275_867e-1),
    (1, 1, -1.695_998_201_029_859e-1),
    (2, 0, -1.075_030_746_475_712_8e-1),
    (0, 3, -2.297_133_060_893_893_2e-2),
    (1, 2, 1.454_508_432_355_709e-1),
    (2, 1, 4.277_364_501_907_745_5e-1),
    (3, 0, 8.949_901_050_811_3e-2),
    (0, 4, 2.124_748_826_587_190_6e-2),
    (1, 3, 1.552_390_955_706_102_5e-1),
    (2, 2, -1.896_547_852_310_150_3e-1),
    (3, 1, -2.626_134_224_530_901e-1),
    (4, 0, -2.647_286_755_190_688e-2),
];

/// The median surface block width on the unit sphere, per unit of grid coordinate x (from the
/// same fit). It sets the radius at which the median block is 1 m wide.
const MAP_MEDIAN_SCALE: f64 = 0.745_944_293_749_470_1;

/// The median surface block width (m). The home planet (65,536 columns per face edge) then has
/// a 43.9 km radius; widths run from about 0.5 m (corners) to 1.08 m (face centres), almost all
/// within a few percent of this.
pub const BLOCK_WIDTH: f64 = 1.0;

/// One of the 6 cube faces: 0 = +X, 1 = -X, 2 = +Y, 3 = -Y, 4 = +Z, 5 = -Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// `chunks_per_face` sets the size: the sea-level radius is chosen so the median surface
    /// block is [`BLOCK_WIDTH`] wide (see [`MAP_Q`]). `height_limit` is how many layers exist
    /// above sea level.
    pub fn new(center: DVec3, chunks_per_face: i32, height_limit: i32, seed: u64) -> Self {
        let face_blocks = chunks_per_face * CHUNK_SIZE;
        // A block spans 2 / N of x, so it is R * (2 / N) * scale wide.
        let sea_radius = face_blocks as f64 * BLOCK_WIDTH / (2.0 * MAP_MEDIAN_SCALE);
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
        let mut above = i32::MAX;
        loop {
            let mut bottom = (planet.layer_at_radius(boundary_radius) / CHUNK_SIZE as f64).round()
                as i32
                * CHUNK_SIZE;
            if res <= 4 * CHUNK_SIZE {
                // Near the centre whole chunks are too coarse a step for that rule (one shell of
                // 128 columns went from 96 m all the way in, its blocks thin slivers): a chunk
                // of layers per halving, 128 columns from 64 m, 64 from 32 m, 32 in the middle.
                // Blocks stay 0.8-1.6 m wide down to 16 m from the centre.
                bottom = res / CHUNK_SIZE / 2 * CHUNK_SIZE;
            }
            bottom = bottom.min(above - CHUNK_SIZE);
            if res / 2 < CHUNK_SIZE || bottom < CHUNK_SIZE {
                planet.shells.push(Shell { bottom: 0, res });
                break;
            }
            planet.shells.push(Shell { bottom, res });
            above = bottom;
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

    /// Grid position (0..=n, fractional allowed) on a face with `n` columns per edge -> the
    /// face coordinate in [-1, 1].
    fn grid_coord(g: f64, n: i32) -> f64 {
        let n = n as f64;
        (2.0 * g - n) / n
    }

    /// [`grid_coord`](Self::grid_coord) of an integer grid line, exact at the face's edges and
    /// centre line.
    fn grid_coord_exact(g: i32, n: i32) -> f64 {
        if g == 0 {
            -1.0
        } else if g == n {
            1.0
        } else if 2 * g == n {
            0.0
        } else {
            Self::grid_coord(g as f64, n)
        }
    }

    /// Face coordinate in [-1, 1] -> grid position (0..=n).
    fn coord_grid(x: f64, n: i32) -> f64 {
        (x + 1.0) * 0.5 * n as f64
    }

    /// A(x, y) of the face map (see [`MAP_Q`]) and its partial derivatives (dA/dx, dA/dy).
    ///
    /// Beyond the face (|x| > 1 or |y| > 1, where physics keeps one face's grid while crossing
    /// an edge) the polynomial would run away. There the map continues *in angle*: the angle
    /// atan(a) grows linearly with x from the edge (pi/4) with the edge's slope, and y's
    /// influence is frozen at the edge. That matches value and slope at the edge, and grid
    /// coordinates stay bounded towards the far side of the planet (like the old tan map), so
    /// sweeps through far-away grid space stay short.
    fn map_axis(x: f64, y: f64) -> (f64, f64, f64) {
        let outside = x.abs() - 1.0;
        let ax = x.abs().min(1.0);
        let inside_y = y.abs() <= 1.0;
        let (xx, yy) = (ax * ax, if inside_y { y * y } else { 1.0 });
        let mut xp = [1.0; 5];
        let mut yp = [1.0; 5];
        for i in 1..5 {
            xp[i] = xp[i - 1] * xx;
            yp[i] = yp[i - 1] * yy;
        }
        let (mut q, mut qx, mut qy) = (0.0, 0.0, 0.0);
        for &(i, j, c) in &MAP_Q {
            q += c * xp[i] * yp[j];
            if i > 0 {
                qx += c * i as f64 * xp[i - 1] * yp[j];
            }
            if j > 0 {
                qy += c * j as f64 * xp[i] * yp[j - 1];
            }
        }
        let qy = if inside_y { qy } else { 0.0 };
        let da_dx = 1.0 + (1.0 - 3.0 * xx) * q + 2.0 * xx * (1.0 - xx) * qx;
        if outside > 0.0 {
            // At the edge A = 1, and (1 - x^2) = 0 leaves dA/dx = 1 - 2 Q: the angle's slope is
            // half that (d atan(a) = da / (1 + a^2), a = 1).
            let slope = 0.5 * da_dx;
            let angle = (FRAC_PI_4 + outside * slope).min(FRAC_PI_2 - 1e-9);
            let a = angle.tan();
            let sec2 = 1.0 + a * a;
            // d slope / dy = -dQ/dy = -2 y Q_Y.
            let da_dy = sec2 * outside * (-2.0 * y * qy);
            return (a.copysign(x), sec2 * slope, da_dy * x.signum());
        }
        let a = if ax == 1.0 {
            1.0
        } else {
            ax * (1.0 + (1.0 - xx) * q)
        };
        let da_dy = 2.0 * x * y * (1.0 - xx) * qy;
        (a.copysign(x), da_dx, da_dy)
    }

    /// Face coordinates (x, y) in [-1, 1]^2 -> cube-face coordinates (a, b).
    pub fn face_to_cube(x: f64, y: f64) -> (f64, f64) {
        (Self::map_axis(x, y).0, Self::map_axis(y, x).0)
    }

    /// Inverse of [`face_to_cube`](Self::face_to_cube), by Newton's method (also a little
    /// beyond the face, where the map continues smoothly).
    pub fn cube_to_face(a: f64, b: f64) -> (f64, f64) {
        // Outside the face the map decouples (see `map_axis`): invert it directly.
        let (out_a, out_b) = (a.abs() > 1.0, b.abs() > 1.0);
        if out_a || out_b {
            // The angle continuation, given the other coordinate (frozen at the edge if it is
            // outside too).
            let beyond = |c: f64, other: f64| {
                let slope = 0.5 * Self::map_axis(1.0, other.clamp(-1.0, 1.0)).1;
                (1.0 + (c.abs().atan() - FRAC_PI_4) / slope).copysign(c)
            };
            return match (out_a, out_b) {
                (true, true) => (beyond(a, 1.0), beyond(b, 1.0)),
                (true, false) => {
                    let y = Self::cube_to_edge(b);
                    (beyond(a, y), y)
                }
                _ => {
                    let x = Self::cube_to_edge(a);
                    (x, beyond(b, x))
                }
            };
        }
        let (mut x, mut y) = (a, b);
        for _ in 0..40 {
            let (fa, ax, ay) = Self::map_axis(x, y);
            let (fb, by, bx) = Self::map_axis(y, x);
            let (ra, rb) = (fa - a, fb - b);
            let det = ax * by - ay * bx;
            let dx = (ra * by - ay * rb) / det;
            let dy = (ax * rb - bx * ra) / det;
            x -= dx;
            y -= dy;
            // Converged to (relative) rounding level.
            if dx.abs() + dy.abs() <= 4e-16 * (1.0 + x.abs() + y.abs()) {
                break;
            }
        }
        (x, y)
    }

    /// The cube coordinate along a face edge (the other coordinate is +-1) at face coordinate
    /// `x`, and its inverse. Both faces sharing the edge use these, so they agree exactly.
    fn edge_to_cube(x: f64) -> f64 {
        Self::map_axis(x, 1.0).0
    }

    fn cube_to_edge(c: f64) -> f64 {
        let mut x = c;
        for _ in 0..40 {
            let (f, dx, _) = Self::map_axis(x, 1.0);
            let step = (f - c) / dx;
            x -= step;
            if step.abs() <= 4e-16 * (1.0 + x.abs()) {
                break;
            }
        }
        x
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
        let (a, b) = Self::face_to_cube(Self::grid_coord(u, n), Self::grid_coord(v, n));
        (normal + s * a + t * b).normalize()
    }

    /// Unit direction through an integer grid corner. Exact on face edges, so neighbouring
    /// faces produce identical vertices (no cracks). A coarse shell's corners also coincide
    /// exactly with every other corner of the finer shell above it.
    pub fn corner_dir(&self, face: Face, u: i32, v: i32, n: i32) -> DVec3 {
        let (normal, s, t) = face.basis();
        let (a, b) = Self::face_to_cube(Self::grid_coord_exact(u, n), Self::grid_coord_exact(v, n));
        (normal + s * a + t * b).normalize()
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
        let (x, y) = Self::cube_to_face(rel.dot(s) / dn, rel.dot(t) / dn);
        let (u, v) = (Self::coord_grid(x, n), Self::coord_grid(y, n));
        (face, u, v, k)
    }

    /// Like [`locate`](Self::locate), but in the grid of a given face and resolution `n`
    /// (coordinates may run outside 0..n near the face's edges). Used to keep one consistent
    /// grid while something crosses a face edge.
    pub fn locate_on_face(&self, face: Face, rel: DVec3, n: i32) -> (f64, f64, f64) {
        let (normal, s, t) = face.basis();
        let k = self.layer_at_radius(rel.length());
        let dn = rel.dot(normal).max(1e-12);
        let (x, y) = Self::cube_to_face(rel.dot(s) / dn, rel.dot(t) / dn);
        let (u, v) = (Self::coord_grid(x, n), Self::coord_grid(y, n));
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
        let along = Self::edge_to_cube(Self::grid_coord(shared as f64 + 0.5, n));
        let p = normal + new_normal + shared_axis * along;
        let edge_cell = |c: f64| if c > 0.0 { n - depth } else { depth - 1 };
        let shared_cell =
            |c: f64| (Self::coord_grid(Self::cube_to_edge(c), n).floor() as i32).clamp(0, n - 1);

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

    /// The inverse map is on hot paths (collision, streaming): it must converge in a few steps.
    #[test]
    fn cube_to_face_converges_quickly() {
        let start = std::time::Instant::now();
        let mut sum = 0.0;
        for i in 0..200_000 {
            let t = i as f64 * 0.000_013;
            let (x, y) = Planet::cube_to_face(t.sin(), (t * 1.7).cos() * 0.9);
            sum += x + y;
        }
        assert!(sum.is_finite());
        let per_call = start.elapsed().as_secs_f64() / 200_000.0;
        assert!(per_call < 2e-6, "{:.2} us per inverse", per_call * 1e6);
    }

    #[test]
    fn face_map_is_exact_symmetric_and_invertible() {
        // Edges stay on the cube's edges, the centre lines stay put.
        for y in [-1.0, -0.7, 0.0, 0.3, 0.999, 1.0] {
            assert_eq!(Planet::face_to_cube(1.0, y).0, 1.0);
            assert_eq!(Planet::face_to_cube(-1.0, y).0, -1.0);
            assert_eq!(Planet::face_to_cube(0.0, y).0, 0.0);
        }
        // Mirrored coordinates give bit-identical (mirrored) results.
        for (x, y) in [(0.3, 0.8), (0.91, 0.12), (0.5, 1.0), (1.0, 0.77)] {
            let (a, b) = Planet::face_to_cube(x, y);
            assert_eq!(Planet::face_to_cube(-x, y), (-a, b));
            assert_eq!(Planet::face_to_cube(x, -y), (a, -b));
            assert_eq!(Planet::face_to_cube(y, x), (b, a));
        }
        // Inverse, also a little beyond the face; monotonic along both axes.
        for i in -10..=10 {
            for j in -10..=10 {
                let (x, y) = (i as f64 * 0.101, j as f64 * 0.101);
                let (a, b) = Planet::face_to_cube(x, y);
                let (x2, y2) = Planet::cube_to_face(a, b);
                assert!((x2 - x).abs() < 1e-12 && (y2 - y).abs() < 1e-12, "{x},{y}");
                let (a2, _) = Planet::face_to_cube(x + 1e-3, y);
                assert!(a2 > a);
            }
        }
        // Far beyond the face (physics near edges, fast or far bodies) it stays invertible.
        for (a, b) in [
            (3.0, 0.2),
            (-40.0, 7.0),
            (1.5, 1.5),
            (0.3, -12.0),
            (1e4, -2e4),
        ] {
            let (x, y) = Planet::cube_to_face(a, b);
            let (a2, b2) = Planet::face_to_cube(x, y);
            assert!(
                (a2 - a).abs() < 1e-9 * a.abs().max(1.0),
                "{a},{b} -> {a2},{b2}"
            );
            assert!(
                (b2 - b).abs() < 1e-9 * b.abs().max(1.0),
                "{a},{b} -> {a2},{b2}"
            );
        }
        for x in [-1.0, -0.4, 0.0, 0.25, 0.999, 1.0] {
            let c = Planet::edge_to_cube(x);
            assert!((Planet::cube_to_edge(c) - x).abs() < 1e-12);
        }
    }

    /// Surface blocks are almost all square and close to 1 m (see [`MAP_Q`]); the rest are
    /// round the cube corners.
    #[test]
    fn surface_blocks_are_nearly_cubes() {
        let p = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let (mut lo, mut hi, mut close, mut total) = (f64::MAX, 0f64, 0, 0);
        let (mut close_widths, mut deviation, mut skew) = (0, 0.0, 0.0);
        let mut squares = 0;
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
                let worst = (wu / BLOCK_WIDTH - 1.0)
                    .abs()
                    .max((wv / BLOCK_WIDTH - 1.0).abs());
                deviation += worst;
                skew += (angle - 90.0).abs();
                if (angle - 90.0).abs() < 3.0 {
                    squares += 1;
                }
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
        // The earlier separable map (tan 38 deg) was 0.84..1.19 m wide, skewed 9 deg on average
        // (up to 30 near corners) and 61% within 10% of 1 m and 10 deg of square.
        assert!(
            lo > 0.45 * BLOCK_WIDTH && hi < 1.1 * BLOCK_WIDTH,
            "widths {lo}..{hi}"
        );
        assert!(close_widths as f64 / total as f64 > 0.75);
        assert!(
            skew / (total as f64) < 1.5,
            "mean skew {}",
            skew / total as f64
        );
        assert!(share > 0.75);
        assert!(
            squares as f64 / total as f64 > 0.94,
            "within 3 deg: {squares}"
        );
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
                // (Near the centre, where a shell is one chunk of layers, a bit wider.)
                let widest = if top - s.bottom <= CHUNK_SIZE {
                    1.6
                } else {
                    1.25
                };
                assert!(width(top) < widest, "shell {i} top width {}", width(top));
                assert!(
                    width(s.bottom) > 0.4,
                    "shell {i} bottom width {}",
                    width(s.bottom)
                );
            }
            // The middle: 32 columns, nothing thinner than half a metre beyond 10 m out.
            let middle = p.shells.last().unwrap();
            assert_eq!(middle.res, CHUNK_SIZE);
            assert!(p.layer_radius(10.0) * PI / 2.0 / middle.res as f64 > 0.45);
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
                let (x, y) = Planet::cube_to_face(d.dot(s2) / dn, d.dot(t2) / dn);
                let (gu, gv) = (Planet::coord_grid(x, n), Planet::coord_grid(y, n));
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
