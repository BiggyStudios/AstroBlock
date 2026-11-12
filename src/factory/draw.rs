//! Drawing a factory: the items riding on conveyors (one mesh per kind of item and light,
//! drawn turned into place along each belt) and the cables (one mesh each, built once). The
//! conveyors, poles and machines themselves are model blocks, drawn with the others.

use anyhow::Result;
use glam::{DMat3, DQuat, DVec3};

use super::{BELT_SPEED, Factory, Node, SPACING};
use crate::{
    render::{
        DrawItem, Fade, MeshHandle, Renderer,
        models::{self, Light},
    },
    survival::items::Item,
    util::FastMap,
    world::{
        chunk::BlockCell, planet::Planet, streaming::WorldStreamer, terrain::TerrainGenerator,
    },
};

/// Items on conveyors are drawn within this distance (m), cables within the second.
const ITEM_DISTANCE: f64 = 48.0;
const CABLE_DISTANCE: f64 = 320.0;

/// The meshes a factory draws with.
#[derive(Default)]
pub struct Art {
    items: FastMap<(Item, u32, u32), MeshHandle>,
    cables: FastMap<(BlockCell, BlockCell), Cable>,
    garbage: Vec<MeshHandle>,
}

struct Cable {
    mesh: MeshHandle,
    origin: DVec3,
    radius: f32,
}

impl Factory {
    /// Where a cable meets a pole or a machine (planet-relative).
    pub fn attach_point(&self, planet: &Planet, cell: BlockCell) -> DVec3 {
        let top = match self.node(cell) {
            Some(Node::Pole(_)) => models::POLE_TOP,
            Some(Node::Machine(i)) => self.machines[i as usize].kind.cable_height(),
            _ => models::MACHINE_TOP,
        };
        models::cell_point(planet, cell, [8.0, 8.0, top])
    }

    /// Lists what to draw: items on conveyors near the camera (`camera` is world space),
    /// and the cables.
    pub fn draw_items(
        &mut self,
        planet: &Planet,
        terrain: &TerrainGenerator,
        streamer: &WorldStreamer,
        renderer: &mut Renderer,
        camera: DVec3,
        draws: &mut Vec<DrawItem>,
    ) -> Result<()> {
        for mesh in self.art.garbage.drain(..) {
            renderer.destroy_mesh(mesh);
        }
        self.locate(planet);
        let camera = camera - planet.center;

        // Cables: built once; those taken away are let go.
        if self.art.cables.len() != self.cables.len()
            || self.cables.iter().any(|c| !self.art.cables.contains_key(c))
        {
            let cables = &self.cables;
            let garbage = &mut self.art.garbage;
            self.art.cables.retain(|key, cable| {
                let keep = cables.contains(key);
                if !keep {
                    garbage.push(cable.mesh);
                }
                keep
            });
            for &(a, b) in &self.cables.clone() {
                if self.art.cables.contains_key(&(a, b)) {
                    continue;
                }
                let (pa, pb) = (self.attach_point(planet, a), self.attach_point(planet, b));
                let length = (pb - pa).length();
                let light = Light { sky: 15, block: 0 };
                let (mesh, origin) = models::cable_model(pa, pb, 0.08 + length * 0.03, light);
                let Some(handle) = renderer.create_mesh(&mesh)? else {
                    break; // (this frame's uploads are used up: the rest next frame)
                };
                let radius = (length * 0.5 + 1.0) as f32;
                self.art.cables.insert(
                    (a, b),
                    Cable {
                        mesh: handle,
                        origin,
                        radius,
                    },
                );
            }
        }
        for cable in self.art.cables.values() {
            if (cable.origin - camera).length() < CABLE_DISTANCE {
                draws.push(DrawItem {
                    mesh: cable.mesh,
                    origin: planet.center + cable.origin,
                    rotation: glam::Quat::IDENTITY,
                    radius: cable.radius,
                    fade: Fade::Visible,
                });
            }
        }

        // Items, carried on between ticks so they glide.
        let ahead = (self.behind as f32 * BELT_SPEED).max(0.0);
        for c in &mut self.conveyors {
            if c.is_empty() {
                continue;
            }
            let Some(place) = c.place else { continue };
            if (place.center - camera).length() > ITEM_DISTANCE {
                continue;
            }
            let sky = *c
                .sky
                .get_or_insert_with(|| crate::survival::sky_light(planet, terrain, Some(c.cell)));
            let block = streamer.light(c.cell) as u32;
            let mut limit = 1.0f32;
            for item in c.items.iter().flatten() {
                let extra = if c.moving { ahead } else { 0.0 };
                let at = (item.at + extra).min(limit).max(item.at.min(limit));
                limit = at - SPACING;
                // (Round a bend, turned the way it is going there.)
                let (position, along) = place.at(at.clamp(0.0, 1.0) as f64);
                let x = along.normalize_or(DVec3::X);
                let z = (place.up - x * place.up.dot(x)).normalize_or(DVec3::Z);
                let rotation = DQuat::from_mat3(&DMat3::from_cols(x, z.cross(x), z)).as_quat();
                let key = (item.stack.item, sky, block);
                let mesh = match self.art.items.get(&key) {
                    Some(&mesh) => mesh,
                    None => {
                        let model = models::belt_item_model(key.0, Light { sky, block });
                        match renderer.create_mesh(&model)? {
                            Some(mesh) => *self.art.items.entry(key).or_insert(mesh),
                            None => continue,
                        }
                    }
                };
                draws.push(DrawItem {
                    mesh,
                    origin: planet.center + position,
                    rotation,
                    radius: 0.4,
                    fade: Fade::Visible,
                });
            }
        }
        Ok(())
    }
}
