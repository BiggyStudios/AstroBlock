//! Player edits to the generated world (broken and placed blocks).
//!
//! Generation stays a pure function of (seed, position); edits are an overlay applied on top
//! whenever a chunk is (re)generated - to the chunk itself *and* to the 1-block padding it
//! borrows from its neighbours, so meshes stay seamless next to edited blocks.
//!
//! Edits live in memory for the session (saving to disk is future work).

use std::sync::RwLock;

use super::{
    block::Block,
    chunk::{BlockCell, ChunkKey, PaddedBlocks},
    planet::{CHUNK_SIZE, Planet},
};
use crate::util::FastMap;

#[derive(Default)]
pub struct WorldEdits {
    /// Per chunk: chunk-local index (u + 32 v + 1024 k) -> block.
    chunks: RwLock<FastMap<ChunkKey, FastMap<u16, Block>>>,
}

fn local_index([u, v, k]: [i32; 3]) -> u16 {
    (u + v * CHUNK_SIZE + k * CHUNK_SIZE * CHUNK_SIZE) as u16
}

impl WorldEdits {
    pub fn set(&self, planet: &Planet, cell: BlockCell, block: Block) {
        let (key, local) = planet.cell_chunk(cell);
        self.chunks
            .write()
            .unwrap()
            .entry(key)
            .or_default()
            .insert(local_index(local), block);
    }

    /// True if `key` or any chunk whose blocks reach into its padding has edits.
    pub fn touches(&self, planet: &Planet, key: ChunkKey) -> bool {
        let chunks = self.chunks.read().unwrap();
        !chunks.is_empty()
            && (chunks.contains_key(&key)
                || neighbour_regions(planet, key).any(|(n, _)| chunks.contains_key(&n)))
    }

    /// Applies the edits that fall inside a padded chunk (the chunk and its 1-block border).
    pub fn apply(&self, planet: &Planet, key: ChunkKey, blocks: &mut PaddedBlocks) {
        let chunks = self.chunks.read().unwrap();
        if chunks.is_empty() {
            return;
        }
        if let Some(edits) = chunks.get(&key) {
            for (&i, &block) in edits {
                let i = i as i32;
                let (u, v, k) = (i % CHUNK_SIZE, (i / CHUNK_SIZE) % CHUNK_SIZE, i / 1024);
                blocks.set(u, v, k, block);
            }
        }
        // Padding: each of the 26 border regions belongs to one neighbouring chunk.
        for (neighbour, region) in neighbour_regions(planet, key) {
            let Some(edits) = chunks.get(&neighbour) else {
                continue;
            };
            for [lu, lv, lk] in region {
                if let Some(cell) = padding_cell(planet, key, [lu, lv, lk]) {
                    let (owner, local) = planet.cell_chunk(cell);
                    if owner == neighbour
                        && let Some(&block) = edits.get(&local_index(local))
                    {
                        blocks.set(lu, lv, lk, block);
                    }
                }
            }
        }
    }
}

/// The cell at padded-local coordinates of a chunk (None across a shell boundary, where the
/// grids differ - edits there don't reach the neighbour's padding).
fn padding_cell(planet: &Planet, key: ChunkKey, [lu, lv, lk]: [i32; 3]) -> Option<BlockCell> {
    let k = key.ck * CHUNK_SIZE + lk;
    let n = planet.res_at(key.ck * CHUNK_SIZE);
    if !(0..planet.layers).contains(&k) || planet.res_at(k) != n {
        return None;
    }
    planet.cell(
        key.face,
        key.cu * CHUNK_SIZE + lu,
        key.cv * CHUNK_SIZE + lv,
        k,
    )
}

/// The 26 padding regions of a chunk, each with the neighbour chunk that owns it.
fn neighbour_regions(
    planet: &Planet,
    key: ChunkKey,
) -> impl Iterator<Item = (ChunkKey, Vec<[i32; 3]>)> + '_ {
    let range = |d: i32| match d {
        -1 => -1..=-1,
        0 => 0..=CHUNK_SIZE - 1,
        _ => CHUNK_SIZE..=CHUNK_SIZE,
    };
    (0..27)
        .map(|i| (i % 3 - 1, (i / 3) % 3 - 1, i / 9 - 1))
        .filter(|&d| d != (0, 0, 0))
        .filter_map(move |(du, dv, dk)| {
            // The owner of the region's first cell owns the whole region (chunk boundaries
            // line up, also across face edges).
            let first = [*range(du).start(), *range(dv).start(), *range(dk).start()];
            let cell = padding_cell(planet, key, first)?;
            let (owner, _) = planet.cell_chunk(cell);
            let cells = range(dk)
                .flat_map(|k| range(dv).flat_map(move |v| range(du).map(move |u| [u, v, k])))
                .collect();
            Some((owner, cells))
        })
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use super::*;
    use crate::world::planet::Face;

    #[test]
    fn edits_reach_neighbour_padding() {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let edits = WorldEdits::default();
        let k = planet.sea_layer + 5;
        let key = ChunkKey {
            face: Face(2),
            cu: 10,
            cv: 10,
            ck: k / CHUNK_SIZE,
        };
        // A block in the neighbouring chunk (+u), right at the shared border.
        let cell = BlockCell {
            face: key.face,
            u: 11 * CHUNK_SIZE,
            v: 10 * CHUNK_SIZE + 3,
            k,
        };
        edits.set(&planet, cell, Block::Stone);
        assert!(edits.touches(&planet, key));
        let mut blocks = PaddedBlocks::new();
        edits.apply(&planet, key, &mut blocks);
        assert_eq!(blocks.get(CHUNK_SIZE, 3, k % CHUNK_SIZE), Block::Stone);
        // A chunk two columns away is not affected.
        let far = ChunkKey { cu: 13, ..key };
        assert!(!edits.touches(&planet, far));
    }
}
