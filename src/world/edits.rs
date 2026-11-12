//! Player edits to the generated world (broken and placed blocks).
//!
//! Generation stays a pure function of (seed, position); edits are an overlay applied on top
//! whenever a chunk is (re)generated - to the chunk itself *and* to the 1-block padding it
//! borrows from its neighbours, so meshes stay seamless next to edited blocks.
//!
//! On disk (`to_bytes`) blocks are stored through a table of their names, so inserting a new
//! block type later never scrambles an old save.

use std::sync::RwLock;

use anyhow::{Result, bail};

use super::{
    block::Block,
    chunk::{BlockCell, ChunkKey, PaddedBlocks},
    planet::{CHUNK_SIZE, Face, Planet},
};
use crate::util::FastMap;

/// Little-endian fields out of a byte slice, failing cleanly on a cut-off file.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(slice) = self.data.get(self.at..self.at + n) else {
            bail!("the file ends too soon");
        };
        self.at += n;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
}

#[derive(Default)]
pub struct WorldEdits {
    /// Per chunk: chunk-local index (u + 32 v + 1024 k) -> block.
    chunks: RwLock<FastMap<ChunkKey, FastMap<u16, Block>>>,
}

fn local_index([u, v, k]: [i32; 3]) -> u16 {
    (u + v * CHUNK_SIZE + k * CHUNK_SIZE * CHUNK_SIZE) as u16
}

const MAGIC: &[u8; 4] = b"ABE1";

impl WorldEdits {
    /// All edits as a compact binary file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let chunks = self.chunks.read().unwrap();
        let palette: Vec<Block> = chunks
            .values()
            .flat_map(|c| c.values().copied())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let edits: usize = chunks.values().map(|c| c.len()).sum();
        let mut out = Vec::with_capacity(16 + palette.len() * 16 + chunks.len() * 15 + edits * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(palette.len() as u16).to_le_bytes());
        for block in &palette {
            let name = format!("{block:?}");
            out.push(name.len() as u8);
            out.extend_from_slice(name.as_bytes());
        }
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for (key, cells) in chunks.iter() {
            out.push(key.face.0);
            for x in [key.cu, key.cv, key.ck] {
                out.extend_from_slice(&x.to_le_bytes());
            }
            out.extend_from_slice(&(cells.len() as u16).to_le_bytes());
            for (&index, block) in cells {
                let id = palette.binary_search(block).expect("in the table") as u16;
                out.extend_from_slice(&index.to_le_bytes());
                out.extend_from_slice(&id.to_le_bytes());
            }
        }
        out
    }

    /// Replaces all edits with those of a file written by `to_bytes` (on a bad file, keeps
    /// what it has). Blocks this version of the game doesn't know are left as generated.
    pub fn read_bytes(&self, data: &[u8]) -> Result<()> {
        let mut r = Reader { data, at: 0 };
        if r.take(4)? != MAGIC {
            bail!("not a block edits file");
        }
        let names = r.u16()? as usize;
        let mut palette = Vec::with_capacity(names);
        for _ in 0..names {
            let len = r.take(1)?[0] as usize;
            let name = std::str::from_utf8(r.take(len)?)?;
            palette.push(
                Block::ALL
                    .iter()
                    .copied()
                    .find(|b| format!("{b:?}") == name),
            );
        }
        let count = r.u32()? as usize;
        let mut chunks: FastMap<ChunkKey, FastMap<u16, Block>> = FastMap::default();
        for _ in 0..count {
            let face = Face(r.take(1)?[0]);
            let (cu, cv, ck) = (r.i32()?, r.i32()?, r.i32()?);
            let cells = r.u16()? as usize;
            let entry = chunks.entry(ChunkKey { face, cu, cv, ck }).or_default();
            for _ in 0..cells {
                let index = r.u16()?;
                let id = r.u16()? as usize;
                let Some(block) = palette.get(id) else {
                    bail!("block edit out of range");
                };
                if let Some(block) = block {
                    entry.insert(index, *block);
                }
            }
        }
        *self.chunks.write().unwrap() = chunks;
        Ok(())
    }

    pub fn set(&self, planet: &Planet, cell: BlockCell, block: Block) {
        let (key, local) = planet.cell_chunk(cell);
        self.chunks
            .write()
            .unwrap()
            .entry(key)
            .or_default()
            .insert(local_index(local), block);
    }

    /// Whether someone has changed this cell (broken or placed something there).
    pub fn edited(&self, planet: &Planet, cell: BlockCell) -> bool {
        let chunks = self.chunks.read().unwrap();
        if chunks.is_empty() {
            return false;
        }
        let (key, local) = planet.cell_chunk(cell);
        chunks
            .get(&key)
            .is_some_and(|c| c.contains_key(&local_index(local)))
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

    #[test]
    fn edits_survive_being_written_and_read() {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let edits = WorldEdits::default();
        let k = planet.sea_layer + 5;
        let cells = [
            (100, 200, Block::Stone),
            (101, 200, Block::Air),
            (9000, 5, Block::Lava),
        ];
        for (u, v, block) in cells {
            let face = Face(4);
            edits.set(&planet, BlockCell { face, u, v, k }, block);
        }
        let bytes = edits.to_bytes();
        let loaded = WorldEdits::default();
        loaded.read_bytes(&bytes).unwrap();
        assert_eq!(
            *loaded.chunks.read().unwrap(),
            *edits.chunks.read().unwrap()
        );
        // A file cut short is an error, not a crash or half a world.
        assert!(loaded.read_bytes(&bytes[..bytes.len() - 3]).is_err());
        assert!(loaded.read_bytes(b"nonsense").is_err());
        assert_eq!(
            *loaded.chunks.read().unwrap(),
            *edits.chunks.read().unwrap()
        );
    }

    /// A block name this version doesn't know keeps the generated block.
    #[test]
    fn unknown_blocks_are_skipped() {
        let planet = Planet::new(DVec3::ZERO, 2048, 1024, 1);
        let edits = WorldEdits::default();
        let k = planet.sea_layer + 5;
        let cell = BlockCell {
            face: Face(4),
            u: 7,
            v: 7,
            k,
        };
        edits.set(&planet, cell, Block::Lava);
        let mut bytes = edits.to_bytes();
        let at = bytes.windows(4).position(|w| w == b"Lava").unwrap();
        bytes[at..at + 4].copy_from_slice(b"Goop");
        let loaded = WorldEdits::default();
        loaded.read_bytes(&bytes).unwrap();
        assert!(loaded.chunks.read().unwrap().values().all(|c| c.is_empty()));
    }

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
