//! Looking at blocks: the ray cast that picks the targeted block, and its outline box.

use glam::{DMat3, DVec3, Mat4, Vec4};

use crate::world::{block::Block, chunk::BlockCell, planet::Planet, streaming::WorldStreamer};

/// Reach (m), as in Minecraft.
pub const SURVIVAL_REACH: f64 = 4.5;
pub const CREATIVE_REACH: f64 = 5.0;

pub struct Hit {
    pub cell: BlockCell,
    pub block: Block,
    /// The empty (or water) cell the ray passed through just before: where a placed block
    /// goes.
    pub before: Option<BlockCell>,
    /// How far along the ray the block was entered (m).
    pub distance: f64,
}

/// Marches a ray (planet-relative origin, unit direction) through the curved block grid.
/// Small fixed steps are simpler than an exact DDA on this grid and plenty at 5 m reach.
pub fn raycast(
    planet: &Planet,
    streamer: &WorldStreamer,
    origin: DVec3,
    dir: DVec3,
    reach: f64,
) -> Option<Hit> {
    const STEP: f64 = 0.02;
    let mut before = None;
    let mut t = 0.0;
    while t <= reach {
        if let Some(cell) = planet.cell_at(origin + dir * t)
            && before != Some(cell)
        {
            match streamer.block(cell) {
                Some(block) if block.is_solid() => {
                    return Some(Hit {
                        cell,
                        block,
                        before,
                        distance: t,
                    });
                }
                Some(_) => before = Some(cell),
                None => return None, // not loaded
            }
        }
        t += STEP;
    }
    None
}

/// The cell's centre (planet-relative).
pub fn cell_center(planet: &Planet, cell: BlockCell) -> DVec3 {
    planet.cell_corners(cell).iter().sum::<DVec3>() / 8.0
}

/// Matrix taking a camera-relative position into the cell's box ([-1, 1]^3 inside), for the
/// outline in `block.frag`. `camera` is the camera's planet-relative position.
pub fn outline_matrix(planet: &Planet, cell: BlockCell, camera: DVec3) -> Mat4 {
    let c = planet.cell_corners(cell);
    let center = c.iter().sum::<DVec3>() / 8.0;
    // Half edge vectors, averaged over the 4 parallel edges (cells are slightly skewed).
    let half = |a: [usize; 4], b: [usize; 4]| {
        (0..4).map(|i| c[b[i]] - c[a[i]]).sum::<DVec3>() / 8.0 * 1.003
    };
    let basis = DMat3::from_cols(
        half([0, 2, 4, 6], [1, 3, 5, 7]),
        half([0, 1, 4, 5], [2, 3, 6, 7]),
        half([0, 1, 2, 3], [4, 5, 6, 7]),
    );
    let inverse = basis.inverse();
    let offset = -(inverse * (center - camera));
    Mat4::from_cols(
        inverse.x_axis.as_vec3().extend(0.0),
        inverse.y_axis.as_vec3().extend(0.0),
        inverse.z_axis.as_vec3().extend(0.0),
        Vec4::new(offset.x as f32, offset.y as f32, offset.z as f32, 1.0),
    )
}
