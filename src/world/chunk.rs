use super::{block::Block, planet::CHUNK_SIZE, planet::Face};

pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Identifies a 32x32x32 chunk on a planet: cube face, chunk column (cu, cv) and chunk
/// layer ck.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub face: Face,
    pub cu: i32,
    pub cv: i32,
    pub ck: i32,
}

/// Block storage of a loaded chunk. Most chunks (open sky, solid rock) are a single block
/// type, which costs 1 byte instead of 32 KiB.
/// One block cell on a planet: face, column (u, v) in the grid of the cell's own shell
/// (`Planet::res_at(k)` columns per face edge) and layer k.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockCell {
    pub face: Face,
    pub u: i32,
    pub v: i32,
    pub k: i32,
}

pub enum ChunkBlocks {
    Uniform(Block),
    Dense(Box<[Block]>),
}

impl ChunkBlocks {
    /// Block at chunk-local (u, v, k), each in 0..32.
    pub fn get(&self, u: i32, v: i32, k: i32) -> Block {
        match self {
            ChunkBlocks::Uniform(b) => *b,
            ChunkBlocks::Dense(blocks) => {
                blocks[(u + v * CHUNK_SIZE + k * CHUNK_SIZE * CHUNK_SIZE) as usize]
            }
        }
    }

    /// Changes one block (chunk-local coordinates).
    pub fn set(&mut self, u: i32, v: i32, k: i32, block: Block) {
        if let ChunkBlocks::Uniform(b) = *self {
            if b == block {
                return;
            }
            *self = ChunkBlocks::Dense(vec![b; CHUNK_VOLUME].into_boxed_slice());
        }
        if let ChunkBlocks::Dense(blocks) = self {
            blocks[(u + v * CHUNK_SIZE + k * CHUNK_SIZE * CHUNK_SIZE) as usize] = block;
        }
    }

    /// Extracts the inner 32^3 of a padded chunk.
    pub fn from_padded(padded: &PaddedBlocks) -> Self {
        let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
        for k in 0..CHUNK_SIZE {
            for v in 0..CHUNK_SIZE {
                for u in 0..CHUNK_SIZE {
                    blocks.push(padded.get(u, v, k));
                }
            }
        }
        if blocks.iter().all(|&b| b == blocks[0]) {
            ChunkBlocks::Uniform(blocks[0])
        } else {
            ChunkBlocks::Dense(blocks.into_boxed_slice())
        }
    }
}

pub const PADDED: i32 = CHUNK_SIZE + 2;

/// A chunk plus a 1-block border of its neighbours (34^3), which is everything the mesher
/// needs for face culling and ambient occlusion - so meshing never touches other chunks.
/// Local coordinates run from -1 to 32 inclusive.
pub struct PaddedBlocks {
    blocks: Vec<Block>,
}

impl PaddedBlocks {
    pub fn new() -> Self {
        Self {
            blocks: vec![Block::Air; (PADDED * PADDED * PADDED) as usize],
        }
    }

    #[inline]
    fn index(u: i32, v: i32, k: i32) -> usize {
        debug_assert!((-1..=CHUNK_SIZE).contains(&u));
        debug_assert!((-1..=CHUNK_SIZE).contains(&v));
        debug_assert!((-1..=CHUNK_SIZE).contains(&k));
        ((u + 1) + (v + 1) * PADDED + (k + 1) * PADDED * PADDED) as usize
    }

    #[inline]
    pub fn get(&self, u: i32, v: i32, k: i32) -> Block {
        self.blocks[Self::index(u, v, k)]
    }

    #[inline]
    pub fn set(&mut self, u: i32, v: i32, k: i32, block: Block) {
        self.blocks[Self::index(u, v, k)] = block;
    }

    /// True if every block (including padding) is opaque - nothing can be visible.
    pub fn all_opaque(&self) -> bool {
        self.blocks.iter().all(|b| b.is_opaque())
    }

    pub fn in_bounds(u: i32, v: i32, k: i32) -> bool {
        let r = -1..=CHUNK_SIZE;
        r.contains(&u) && r.contains(&v) && r.contains(&k)
    }
}
