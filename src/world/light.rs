//! Block light, Minecraft style: light sources (torches, lit furnaces) flood their light
//! through non-opaque cells, one level less per step, so walls cast shadows and light
//! creeps around corners. Levels run from 15 down to 1.
//!
//! The sources are few and placed by the player, so each one keeps its own flood fill, and
//! `LightMap` holds the brightest level per cell. The map is shared with the streaming
//! workers, which read it while meshing (light goes into each vertex). A change only
//! re-floods the sources whose light touches it, and returns the cells whose level changed,
//! so the streamer can remesh exactly the chunks that show them.
//!
//! Sky light is still the mesher's estimate from the depth below the column's surface.

use std::{
    collections::VecDeque,
    sync::{PoisonError, RwLock},
};

use glam::DVec3;

use super::{chunk::BlockCell, planet::Planet};
use crate::util::{FastMap, FastSet};

pub const MAX_LIGHT: u8 = 15;
/// No light reaches further than this from its source (m).
const REACH: f64 = 20.0;

/// The block light level of every lit cell.
#[derive(Default)]
pub struct LightMap {
    cells: RwLock<FastMap<BlockCell, u8>>,
    /// Planet-relative positions of the sources.
    sources: RwLock<Vec<DVec3>>,
}

impl LightMap {
    pub fn get(&self, cell: BlockCell) -> u8 {
        let cells = self.cells.read().unwrap_or_else(PoisonError::into_inner);
        cells.get(&cell).copied().unwrap_or(0)
    }

    /// Whether any light can reach a sphere (planet-relative centre, radius in meters). Lets
    /// the mesher skip lookups almost everywhere.
    pub fn reaches(&self, center: DVec3, radius: f64) -> bool {
        let sources = self.sources.read().unwrap_or_else(PoisonError::into_inner);
        sources
            .iter()
            .any(|s| (*s - center).length() < radius + REACH)
    }
}

struct Source {
    position: DVec3,
    /// This source's own flood fill.
    lit: FastMap<BlockCell, u8>,
}

#[derive(Default)]
pub struct Lighting {
    pub map: std::sync::Arc<LightMap>,
    sources: FastMap<BlockCell, Source>,
}

impl Lighting {
    /// Adds (level > 0), changes or removes (0) the light source in `cell`. `passes` says
    /// whether light goes through a cell. Returns the cells whose light changed.
    pub fn set_source(
        &mut self,
        planet: &Planet,
        cell: BlockCell,
        level: u8,
        passes: &dyn Fn(BlockCell) -> bool,
    ) -> Vec<BlockCell> {
        let mut touched = FastSet::default();
        let mut near = vec![planet_position(planet, cell)];
        if let Some(old) = self.sources.remove(&cell) {
            touched.extend(old.lit.into_keys());
        }
        if level > 0 {
            let lit = flood(planet, cell, level.min(MAX_LIGHT), passes);
            touched.extend(lit.keys().copied());
            let position = planet_position(planet, cell);
            near.push(position);
            self.sources.insert(cell, Source { position, lit });
        }
        self.merge(touched, &near)
    }

    /// A block changed (placed or removed): re-floods the sources whose light reaches it or
    /// its neighbours. Returns the cells whose light changed.
    pub fn block_changed(
        &mut self,
        planet: &Planet,
        cell: BlockCell,
        passes: &dyn Fn(BlockCell) -> bool,
    ) -> Vec<BlockCell> {
        let mut around = neighbours(planet, cell);
        around.push(cell);
        let affected: Vec<BlockCell> = self
            .sources
            .iter()
            .filter(|(_, s)| around.iter().any(|c| s.lit.contains_key(c)))
            .map(|(&c, _)| c)
            .collect();
        let mut touched = FastSet::default();
        let mut near = Vec::new();
        for id in affected {
            let source = self.sources.get_mut(&id).expect("listed above");
            let level = source.lit.get(&id).copied().unwrap_or(0);
            touched.extend(source.lit.keys().copied());
            source.lit = flood(planet, id, level, passes);
            touched.extend(source.lit.keys().copied());
            near.push(source.position);
        }
        self.merge(touched, &near)
    }

    /// Writes the brightest level of every touched cell into the shared map.
    fn merge(&mut self, touched: FastSet<BlockCell>, near: &[DVec3]) -> Vec<BlockCell> {
        if touched.is_empty() {
            return Vec::new();
        }
        // Only sources near the changed ones can light the same cells.
        let candidates: Vec<&Source> = self
            .sources
            .values()
            .filter(|s| {
                near.iter()
                    .any(|p| (s.position - *p).length() < 2.0 * REACH)
            })
            .collect();
        let mut cells = self
            .map
            .cells
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let mut changed = Vec::new();
        for cell in touched {
            let level = candidates
                .iter()
                .filter_map(|s| s.lit.get(&cell))
                .max()
                .copied()
                .unwrap_or(0);
            let old = cells.get(&cell).copied().unwrap_or(0);
            if level != old {
                if level == 0 {
                    cells.remove(&cell);
                } else {
                    cells.insert(cell, level);
                }
                changed.push(cell);
            }
        }
        drop(cells);
        *self
            .map
            .sources
            .write()
            .unwrap_or_else(PoisonError::into_inner) =
            self.sources.values().map(|s| s.position).collect();
        changed
    }
}

fn planet_position(planet: &Planet, cell: BlockCell) -> DVec3 {
    planet.cell_corners(cell).iter().sum::<DVec3>() / 8.0
}

/// Breadth-first flood fill from `origin`, one level less per step.
fn flood(
    planet: &Planet,
    origin: BlockCell,
    level: u8,
    passes: &dyn Fn(BlockCell) -> bool,
) -> FastMap<BlockCell, u8> {
    let mut lit = FastMap::default();
    let mut queue = VecDeque::new();
    lit.insert(origin, level);
    queue.push_back(origin);
    while let Some(cell) = queue.pop_front() {
        let l = lit[&cell];
        if l <= 1 {
            continue;
        }
        for next in neighbours(planet, cell) {
            if lit.get(&next).is_some_and(|&have| have >= l - 1) || !passes(next) {
                continue;
            }
            lit.insert(next, l - 1);
            queue.push_back(next);
        }
    }
    lit
}

/// The (up to) 6 cells sharing a face with `cell`, across face edges and shell boundaries
/// (where the grid halves, one representative cell is used).
pub fn neighbours(planet: &Planet, c: BlockCell) -> Vec<BlockCell> {
    let n = planet.res_at(c.k);
    let mut out = Vec::with_capacity(6);
    for (du, dv) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        out.extend(planet.cell(c.face, c.u + du, c.v + dv, c.k));
    }
    for dk in [1, -1] {
        let k = c.k + dk;
        if !(0..planet.layers).contains(&k) {
            continue;
        }
        let m = planet.res_at(k);
        let (u, v) = match m.cmp(&n) {
            std::cmp::Ordering::Equal => (c.u, c.v),
            std::cmp::Ordering::Greater => (c.u * 2, c.v * 2),
            std::cmp::Ordering::Less => (c.u / 2, c.v / 2),
        };
        out.extend(planet.cell(c.face, u, v, k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::planet::Face;

    fn cell(u: i32, v: i32, k: i32) -> BlockCell {
        BlockCell {
            face: Face(4),
            u,
            v,
            k,
        }
    }

    #[test]
    fn floods_around_walls_and_relights() {
        let planet = Planet::new(DVec3::ZERO, 64, 128, 1);
        let k = planet.sea_layer + 2;
        // A wall at u = 105 except for a gap at v = 102.
        let wall = std::cell::Cell::new(true);
        let passes = |c: BlockCell| !(c.u == 105 && c.v != 102 && wall.get());
        let mut lighting = Lighting::default();
        let torch = cell(100, 100, k);
        let changed = lighting.set_source(&planet, torch, 14, &passes);
        assert!(!changed.is_empty());
        let map = lighting.map.clone();
        assert_eq!(map.get(torch), 14);
        assert_eq!(map.get(cell(103, 100, k)), 11);
        // Behind the wall, light only arrives through the gap: the long way round.
        assert_eq!(map.get(cell(106, 100, k)), 14 - 7 - 1 - 2);
        // Removing the wall lets it straight through.
        wall.set(false);
        let changed = lighting.block_changed(&planet, cell(105, 100, k), &passes);
        assert!(changed.contains(&cell(106, 100, k)));
        assert_eq!(map.get(cell(106, 100, k)), 8);
        // Removing the torch darkens everything.
        lighting.set_source(&planet, torch, 0, &passes);
        assert_eq!(map.get(cell(103, 100, k)), 0);
        assert!(!map.reaches(planet_position(&planet, torch), 1.0));
    }
}
