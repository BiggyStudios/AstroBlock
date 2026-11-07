# Astro Block

A voxel space game that is also its own engine (there is deliberately no separate engine crate).
The goal is Minecraft-style planets made of blocks combined with Kerbal Space Program-style
rocketry: mine resources, build rockets out of blocks, fly them under Keplerian orbital mechanics,
and eventually go interstellar.

- **Language / API:** Rust (edition 2024) + raw Vulkan 1.3 through `ash`. This was chosen on
  purpose; do not switch to wgpu or another abstraction.
- **Not a Minecraft clone (important):** only the blocks and the blocky, pixel-textured art
  style are shared. The UI, HUD (health, hotbar, status bars), screens, font, icons and
  presentation must be the game's own design. When a request mentions Minecraft it means the
  kind of feature, not its look or exact numbers. The current survival UI and some rule
  values still copy Minecraft and are to be redesigned.
- **Workflow:** the project owner directs development one step at a time. Implement what is asked,
  suggest next steps, and don't start big unrequested systems (rockets, inventory, saving...) on your own.

## Commands

```sh
cargo run --release              # play (debug builds work too, but worldgen is ~5x slower)
cargo test --release             # unit tests (planet seams, mesh validity, biome distribution)
cargo test --release locate_biomes -- --ignored --nocapture   # prints an ASTRO_SPAWN for every biome
cargo clippy --release --all-targets && cargo fmt               # keep both clean
```

`[profile.dev]` uses opt-level 1 for this crate and 3 for dependencies.
Shaders are GLSL in `shaders/`. `build.rs` compiles them to SPIR-V with **naga**, so no Vulkan SDK
or `glslc` is needed. A single source can produce several variants through defines (see the
`SHADERS` table in `build.rs`).

**Controls:** click to capture the mouse, Esc to release it. The ` key opens the debug console
(`help` lists the commands: `gamemode`, `tp`, `orbit`, `launch`, `time`, `warp`, `drag`, ...).
- **Moving:**
  - Survival / creative: WASD walks, Space jumps, Ctrl sprints. Double-tapping Space toggles
    flight in creative (Space/Shift to rise and sink).
  - Spectator: free fly through terrain. Space/Shift move along the radial "up", Ctrl gives
    10x speed, and the scroll wheel scales the fly speed.
- **Blocks and items (Minecraft's controls):**
  - Left click breaks a block (in survival this takes time and depends on the tool), or
    swings at nothing.
  - Right click places the held block, or uses a crafting table, furnace or chest (hold Shift
    to place against it instead). With armor in hand it puts it on. Holding it with food in
    hand eats (1.6 s).
  - Torches go on top of blocks or on their sides, and fall off when that block breaks.
  - Middle click picks the targeted block. 1-9 or the scroll wheel choose the hotbar slot.
  - Q drops one of the held item, Ctrl+Q the whole stack.
  - E opens the inventory (the creative catalog in creative mode); E or Esc closes it.
- **In inventory screens:**
  - Left click picks up, puts down, merges or swaps a stack. Right click picks up half or puts
    down one.
  - Shift-click moves a stack across: between a chest and the inventory, into a furnace's
    input or fuel slot, onto the armor slots, or between hotbar and main inventory. On a
    crafting result it crafts as many as fit.
  - Dragging with a stack held spreads it over the slots you cross: right-drag puts one
    in each, left-drag splits it evenly (the rest stays held). Double-click gathers that item
    from everywhere onto the held stack.
  - 1-9 over a slot swaps it with that hotbar slot. Q over a slot drops from it.
  - **Middle click sorts the inventory.** Clicking outside the panel drops the held stack.
- **Time warp:** `.` and `,` raise and lower it (x1 up to x1,000,000), as in KSP.
- **On screen:** the HUD shows mode, time, warp, altitude, surface and orbital speed, gravity,
  Pe/Ap and the targeted block. The window title shows:
  - FPS and CPU / streaming / GPU-wait timings
  - chunk counts, LOD tiles and mesh VRAM
  - altitude, latitude/longitude, biome, game mode, local solar time and the time warp

### Debug environment variables

| Variable | Effect |
|---|---|
| `ASTRO_SPAWN=lat,lon[,alt,pitch,yaw]` | Spawn at a location (degrees; eye height in meters above sea level; default on the ground). |
| `ASTRO_TIME=<hours>` | Local solar time at the spawn point (default 9). |
| `ASTRO_WARP=<factor>` | Initial time warp. |
| `ASTRO_FACE_SUN=1` | Turn the camera towards the sun's compass direction (sunset shots). |
| `ASTRO_MODE=<mode>` | Starting game mode: survival (default), creative or spectator. **Use `spectator` for camera captures**: in survival you fall. |
| `ASTRO_COMMANDS="cmd;cmd"` | Console commands run 4 s after start, one per frame (e.g. `orbit 20`, `break`, `look`). For survival tests: `aim <pitch> [yaw]`, `give <item> [n]`, `select <1-9>`, `place` (= right click: places, opens, eats, wears), `throw`, `open inventory/creative`, `summon <cow|pig|sheep>`. Captures are silent (no audio device is opened). |
| `ASTRO_CONSOLE=1` | Start with the console open. |
| `ASTRO_VSYNC=0` | Uncapped framerate (IMMEDIATE present mode) for profiling. |
| `ASTRO_AUTOFLY=<m/s>` | Camera flies forward on its own (streaming stress test). |
| `ASTRO_CAPTURE=path.bmp;<secs>` | Unattended mode: the window opens **without focus**, input is ignored, one frame is saved as BMP after `<secs>`, then the game exits. The stats are logged as `capture: ...`. |
| `RUST_LOG=astro_block=debug` | Streaming timings (recompute/unload/uploads) and arena growth. |

**Automated visual checks must use `ASTRO_CAPTURE`.** Never bring the game window to the
foreground or leave a normally launched instance running: the owner may be typing, and their
keystrokes will move the camera. Make sure no `astro_block` process is left behind.

## Architecture

```
src/
  main.rs, app.rs   winit event loop, Game (player, time + warp, sun, exposure, screens, console commands)
  player.rs         Player: inertial-frame physics, voxel collision, modes, rails warp, fall height
  interact.rs       ray cast to the targeted block, its outline matrix
  survival.rs       Survival: inventory, armor, held item, mining/placing, torches, drops, models, furnaces, chests,
                    mobs (spawning, combat), screens' click rules (drag, double-click), health, hunger, sound events
  mobs.rs           passive animals (cow, pig, sheep): wander / panic AI, physics, hits, loot
  audio.rs          sound effects synthesized in code; a small mixer on a cpal stream, panned by position
  items.rs          Item, ItemStack, tools (kind x tier), Minecraft mining rules, block drops, fuel, smelting
  inventory.rs      36-slot Inventory, add/sort, Minecraft slot clicks
  crafting.rs       shaped recipes (mirroring, "any planks")
  furnace.rs        Furnace: fuel and smelting, steps from event to event (works under time warp)
  drops.rs          items lying in the world: physics, pickup radius, despawn
  models.rs         box models (crafting table, furnace, chest, torch, dropped items, mobs) -> ChunkMesh
  screen.rs         inventory (with armor) / crafting / furnace / chest / creative screens: layout, drawing, tooltips
  console.rs        debug console (line editor, history, help text)
  hud.rs, ui.rs     overlay: HUD, hotbar, hearts, console; immediate-mode quads, text, isometric block icons, sprites
  camera.rs         f64 position + planet-relative "up" frame (handles flips at the centre), reverse-Z projection
  input.rs          per-frame key/mouse state
  util.rs           FxHash FastMap/FastSet (use these for hot hash maps keyed by ChunkKey etc.)
  space/
    mod.rs          SolarSystem (star, planet orbit/spin/tilt), RotatingFrame (body <-> inertial), local time
    orbit.rs        Keplerian orbits: elements <-> state vectors (Kepler's equation)
    gravity.rs      PlanetGravity: layered-density gravity inside and out, air density
  render/
    mod.rs          Renderer: frames in flight, uploads, culling, scene passes, atmosphere pass, overlay, screenshots
    atmosphere.rs   Atmosphere model + CPU-precomputed LUTs (transmittance, multiple scattering, sky irradiance)
    starfield.rs    Milky Way map (precomputed equirectangular texture)
    font.rs         built-in 5x7 bitmap font atlas
    context.rs      instance/device (Vulkan 1.3: dynamic rendering + synchronization2), submit_and_wait
    swapchain.rs    swapchain, D32 reverse-Z depth buffer (sampled), HDR colour target (RGBA16F)
    pipeline.rs     opaque / water / far (LOD + fading chunks) / sky (full-screen) / ui pipelines, one layout
    arena.rs        VertexArena (mesh memory in 64 MiB pages, power-of-two blocks) + StagingRing
    mesh.rs         BlockVertex (12 bytes), UiVertex, PushConstants (96 bytes), ChunkMesh, DrawItem, Fade
    texture.rs      block texture array with CPU-generated mips; single-mip 2D textures (LUTs, font)
    frustum.rs      frustum planes for camera-relative bounding spheres
  world/
    planet.rs       solid cube-sphere: Face, Shell, Planet, cells (cell/cell_at/cell_chunk), wrap_column, chunk_neighbors
    chunk.rs        ChunkKey, BlockCell, ChunkBlocks (Uniform | Dense), PaddedBlocks (34^3)
    edits.rs        WorldEdits: player edits, applied on top of generation (chunk + padding)
    light.rs        block light: per-source flood fills, shared LightMap read by the meshing workers
    terrain.rs      TerrainGenerator: columns/climate/biomes, caves, ores, trees, column cache, deep interior
    mesher.rs       greedy mesher on the curved grid, AO, smooth block light, fake sky light, glass cutouts
    streaming.rs    WorldStreamer: worker threads, chunk desired set + LOD tiles, uploads, block access/edits
    lod.rs          LOD terrain quadtree: tile selection, horizon culling, voxel-style tile meshes
    noise.rs        f64 3D simplex (hash-based, never tiles), fbm, ridged
    block.rs        Block enum, textures per face, tint kind, opaque / solid / model, icon faces, names
    textures.rs     procedurally generated 16x16 textures: blocks, then item / tool / interface sprites (Tex enum = array layer)
shaders/
  block.vert/.frag  blocks and models (variants WATER, FAR; FAR also does cutout sprites); HDR lighting from the atmosphere LUTs; target outline
  atmosphere.glsl   shared: frame uniforms, LUT access, medium (#include'd; build.rs resolves includes)
  fullscreen.vert + sky.frag   atmosphere ray march, stars, sun, glare, exposure + ACES tone mapping
  ui.vert/.frag     overlay: font atlas, isometric block icons or item sprites, alpha blended over the final image
```

### Core invariants: don't break these

1. **f64 world, camera-relative rendering.** Every world position is a `DVec3`. Per draw, the CPU
   computes `origin - camera` in f64 and only then converts it to f32. The view matrix is
   rotation-only. The home planet is deliberately placed about 1 AU from the origin
   (`PLANET_CENTER`), so any f32 world coordinate breaks visibly.
2. **Reverse-Z with an infinite far plane:** depth is cleared to 0 and compared with
   `GREATER_OR_EQUAL`. Use `glam::camera::rh::proj::vulkan::*`. In glam 0.34 the old
   `Mat4::perspective_*` / `look_to_*` methods are gone, and the Vulkan variant already flips Y,
   so don't flip it again. naga's `ADJUST_COORDINATE_SPACE` is disabled in `build.rs` for the same
   reason.
3. **Cube-sphere addressing; the planet is a solid ball.** A block is `(face, u, v, k)`. Layer
   `k` is radial and starts at the planet's centre (`k = 0`), with
   `layer_radius(k) = sea_radius + (k - sea_layer)`; `sea_layer = floor(sea_radius)`, and there
   are `HEIGHT_LIMIT` layers above sea level.
   - **Shells:** columns are mapped equiangularly (tangent) onto the sphere, and the grid
     resolution depends on depth (`Planet::shells`). The surface shell has
     `face_blocks = chunks_per_face * 32` columns per face edge. Each deeper shell halves the
     resolution (2x2 columns become 1) where blocks have shrunk to about 0.5 m, so widths stay
     about 0.5-1 m down to the small innermost shell, which tapers to the centre.
   - **Shell boundaries** are multiples of 32 layers, so a chunk belongs to exactly one shell.
     Always get the resolution from `res_at(k)`, and pass `n` explicitly to `dir`,
     `corner_dir`, `wrap_column`, `grid_to_cube` and `cube_to_grid`. `column_dir` means
     surface columns. `locate` returns `u` and `v` in the grid units of the shell at that
     radius.
   - **Neighbours across a shell boundary:** a coarse chunk has 4 finer chunks above it, and a
     fine chunk has 1 coarse chunk below it (`chunk_neighbors`). A coarse grid's corners
     coincide bit-exactly with every other corner of the finer grid above it.
   - **Handedness:** the `(u, v, k)` frame is right-handed (`s x t = normal`).
   - **Face edges:** neighbours across face edges have rotated grids, so always go through
     `wrap_column` / `chunk_neighbors`. `grid_to_cube` is computed symmetrically and
     `corner_dir` is exact at edges, which makes shared vertices bit-identical across faces.
   - **Block shape:** blocks are always 1 m tall, but no grid can cover a sphere with equal
     cubes. The mapping (`MAP_ANGLE` = 38 degrees, plus the radius chosen in `Planet::new`)
     balances the width error:
     - Surface blocks are 0.84-1.19 m wide: widest at face centres, narrowest along face edges.
       They are 9% off on average, and 61% are within 10% of 1 m.
     - The plain equiangular map was 0.71-1.0 m wide.
     - Away from a face's centre lines, blocks are also skewed (9 degrees on average), up to
       120-degree diamonds at the 8 cube corners. The grid lines are great circles, so no
       separable mapping changes that.
     - `find_spawn` picks land where blocks are within a few percent of 1 m cubes.
     - Tests: `surface_blocks_are_nearly_cubes`, `spawn_blocks_are_square`.
   - Tests cover all of the above.
4. **Determinism.** Generation is a pure function of (seed, position). Any thread can generate any
   chunk, and neighbours must agree on their shared border:
   - Cave noise is sampled on a lattice that is global per face (every 4 blocks) and trilinearly
     interpolated.
   - Padding cells across a face seam use `cave_values_slow` with wrapped coordinates.
   - Trees are hashed per column and never placed within 6 columns of a face edge.
   - Ore veins stay inside their own 8^3 cell.
5. **Padded meshing.** The generator fills a 34^3 `PaddedBlocks`, and the mesher only reads that,
   never other chunks.
   - **Block edits** are an overlay (`WorldEdits`) that `generate_chunk` applies after
     generation, to the chunk and to its padding.
     - Every (re)generation therefore sees the same blocks as its neighbours, and an edited
       chunk is simply regenerated.
     - `WorldStreamer::set_block` updates the stored blocks at once (for collision) and bumps
       the chunk's version. It then queues priority jobs for the chunk and every neighbour
       whose padding holds the cell.
     - Results from before the edit are discarded by version.
   - Edits don't cross shell boundaries in padding, and they live in memory only (no saving
     yet).
   - **Deep interior:** anything below `surface_floor()` (sea level - `SURFACE_DEPTH`, 1 km),
     which includes every inner shell, is generated by `deep_chunk` / `fill_deep`. It has radial
     material layers plus caves and no column data. The mesher takes `Columns::Deep`, which
     means white tint and darkness-based fake light. Chunks whose padded blocks are all opaque
     skip meshing.
   - **Padding across a shell boundary** (implemented in `fill_deep`):
     - The cell below a fine chunk is the coarse cell `(u/2, v/2)`.
     - The cell above a coarse chunk counts as open if any of the 4 fine cells over it is open.
       The coarse top face is then drawn whenever something could see it, which leaves no holes
       (at the cost of slight overdraw).
     - Once editing exists, this same rule must be applied to stored blocks.
6. **Vertex positions are snapped to a global lattice** (`POSITION_UNIT = 1/512 m`, stored as i16
   relative to a lattice-aligned origin). Because chunks share the lattice, shared corners are
   identical and there are no cracks. Opaque quads are grown by `QUAD_GROW` to hide T-junction
   cracks from greedy merging. Greedy merges are capped at 8 blocks tangentially (sagitta is
   below 0.2 mm) and are unlimited radially.
7. **GPU resource lifetime.** `Renderer::create_mesh` returns `Ok(None)` when this frame's staging
   budget is full; the caller retries next frame. Meshes freed with `destroy_mesh` go to the
   current frame slot's garbage list and are released once that slot's fence signals. All draws
   use one shared quad index buffer, so meshes must be quads (4 vertices each).

### Vertex / shader conventions
- `BlockVertex { position: [i16; 4], data: u32 }`:
  - `position.w` is the tint as RGB565 of sqrt(linear colour).
  - `data` packs, by bit range: texture layer (bits 0-7), uv corner (8-9), AO (10-11),
    sky light (12-15), and quad size - 1 in u (16-20) and v (21-25).
  - Bit 26 (`models::MODEL_FLAG`): bits 16-25 hold this vertex's own uv in texels instead.
    Models use it for Minecraft-style texture mapping.
  - Bit 27 (`CUTOUT_FLAG`): alpha is transparency (discard below 0.5, no tint). The discard
    exists only in the FAR variant, so models and drops are drawn with the clip pipeline
    (`objects` in `Renderer::render`), and the main opaque pipeline keeps early-z.
  - Bits 28-31: block light 0-15 (torches; see `world::light`). Models and mobs carry it too.
- `ChunkMesh` has three sections: `opaque`, `water` and `cutout` (glass; alpha-tested, drawn
  with the clip pipeline after the opaque pass).
- The texture layer reaches the fragment shader as a `flat uint`. If it were an interpolated
  float, naga would truncate it, and `4.0` would sometimes come out as layer 3.
- naga GLSL requires separate `texture2DArray` + `sampler` bindings (no combined samplers).
- naga 30 has a feature-gating bug: `glsl-in` only compiles if `wgsl-in` is also enabled. That is
  why `Cargo.toml` enables it.
- Normals are not stored. The fragment shader derives flat normals from `dFdx/dFdy` of the
  camera-relative position.
- Texture alpha is the biome tint mask for opaque blocks and opacity for water. For item
  sprites (`Tex::Stick` onwards) it is transparency.
- UI vertices: layer `u32::MAX` means the font atlas. Otherwise bits 0-7 are the layer, bits
  8-15 a brightness (isometric icon faces), and bit 31 marks a sprite (`ui.rs` and `ui.frag`
  must agree).

### World generation overview (terrain.rs)
- `sample_column(dir)` builds each column from these layers:
  - continentalness spline
  - ridged mountain belts
  - hills and detail noise
  - temperature from latitude (+Y is the spin axis) minus altitude cooling, and humidity
  - deserts with dunes, badlands with terraced mesas, temperate terraced cliffs
  - a biome classification (14 biomes)
- Column data is cached per chunk-column (`OnceLock` entries in a `Mutex<FastMap>`, evicted by
  epoch through `maintain_cache`).
- Ores follow loose real geology:
  - coal under forests and plains
  - copper in arid areas and mountains
  - gold and uranium in mountains
  - diamonds deep under cold cratons
  - bauxite in tropical soil
  - iron everywhere, plus large "iron provinces"
  - ilmenite (titanium) placer sand on some beaches
- Caves exist at every depth, down to the centre. There are two kinds: spaghetti tunnels (two
  noise fields both near zero) and "cheese" caverns.
  - `cave_noise(face, u, v, k, n)` uses parameters that change smoothly with radius, so there
    are no seams between depth zones:
    - Near the surface, caves are flattened radially (mostly horizontal tunnels), fading out
      between 2 and 6 km depth.
    - The noise scale grows with depth, so tunnels get wider and caverns huge.
  - `is_cave` thresholds also depend on depth: caverns are most common in the mantle and rare
    in the core.
  - Near the sea floor, caves are suppressed so they don't open into the ocean.
  - The cave lattice is per face *and per shell* (it takes `n`), so every chunk agrees on
    shared cells.
- The interior (`deep_block`) is layered by radius:
  - Deepstone crust, down to `CRUST_DEPTH` = 3 km
  - Mantle, down to 0.55 R
  - molten OuterCore, down to 0.2 R
  - iron-nickel InnerCore at the centre
- There is no bedrock: the planet is solid all the way through.

### Streaming (streaming.rs)
- Load radius is 12 chunks (384 m). The desired set is a flood fill from the camera's chunk.
  Chunks below the local terrain floor get a distance penalty (`UNDERGROUND_PENALTY`), so only
  about 130 m of caves load beneath you.
- A pending queue (nearest first) feeds a bounded number of in-flight jobs on `cores - 2` worker
  threads (crossbeam channel). At most 48 uploads happen per frame. Unloading has a hysteresis
  margin.
- Workers take a `Job` (`Chunk` or `Lod`) and return the matching `JobResult`. LOD tiles have
  their own in-flight budget (workers x 2) and upload budget (24 per frame).
- `loaded_radius()` drives the LOD discard radius, which overlaps the loaded area by 32 m.
- When the camera is more than 24 m below the surface above it (`camera_underground`), the
  desired set becomes a plain sphere of `UNDERGROUND_RADIUS` (192 m), and the app switches to a
  dark sky with matching fog.
- `block_at(rel)` looks up the block at a position in the loaded chunks. The app uses it to make
  the sky and fog dark while the camera is inside solid rock (fog closes to 96 m).

### LOD terrain (lod.rs)
- Every cube face has a quadtree of `LodKey { face, level, tu, tv }` tiles. A tile has
  `LOD_CELLS` (32) x 32 cells, and each cell is `2^level` surface columns wide. A tile splits
  while the camera is closer than `LOD_SPLIT_DISTANCE` (5) tile widths. Tiles beyond the
  horizon (accounting for the tile's highest point) are culled in `tile_distance`.
- `build_tile` samples one `sample_column` per cell (34x34 including a border) and meshes in the
  voxel style: block-column tops, walls down to lower neighbours, and skirts at tile edges to hide
  cracks between levels. Steep cells get rock/stone tops, and water is a separate translucent
  layer at exactly sea level.
- **Trees:** levels up to `TREE_LEVELS` (2) draw the *real* trees. `TerrainGenerator::tree_at`
  returns exactly the tree the chunk generator plants in a column (shared `tree_roll` /
  `tree_kind` / `tree_height`; test `tree_at_matches_chunk_trees`). Level 1 uses detailed box
  crowns, level 2 a single box. LOD trees are shrunk by `TREE_SHRINK` so they hide inside the
  real trees. Coarser levels draw canopy cells instead, with coverage = the area the real crowns
  cover (1 - exp(-density x crown footprint)). Keep those numbers in sync if tree densities change.
- Opaque LOD geometry sits `SINK` (0.25 m) below the real terrain so chunks win the depth test
  where both are drawn. The vertex origin is at the tile's mid height, so the i16 range covers
  mountains.
- Greedy merging of tops is capped by `max_merge` so a flat quad's sag (sagitta) stays under
  about 5% of a cell. Without it, ocean quads tens of km wide sink below the sea floor and show
  up as grey patches from orbit.
- The streamer reselects tiles after the camera moves 16 m (`reselect_lod`). `resolve_lod`
  only replaces a tile by its children once all 4 are generated, and a desired tile that is
  still generating is stood in for by loaded finer tiles. So there are no holes or overlaps.
- **Cross-fades:** tiles that change visibility fade with a screen-space dither over
  `LOD_FADE` (0.5 s) (`lod_visible`). The outgoing and incoming tiles of a swap use the same
  progress, and the shader keeps complementary pixels. New chunks fade in over `CHUNK_FADE`
  (0.35 s), and unloaded chunks fade out (`retiring`). Chunks that are still fading don't count
  as loaded for `loaded_radius`.
- **Rendering:** the clip pipelines (`far` = `block.frag` with `FAR`, and water) discard
  fragments closer than `offset.w` or further than `params.y`, and apply the fade
  (`params.zw`, see `Fade`). Pass order:
  1. steady chunks (opaque pipeline, no discard, keeps early-z)
  2. fading chunks and LOD tiles (inner radius `loaded_radius() - 32 m`)
  3. LOD water (same inner radius)
  4. chunk water (outer radius = that same radius)

  Chunk water and LOD water therefore meet at one radius and never stack (stacking showed up as
  a band in the ocean). LOD is not drawn while the camera is underground; the radius is then
  infinity.

### Sun, time and frames (space/)
- **The world frame is the home planet's body-fixed (rotating) frame**, centred on
  `PLANET_CENTER`, with the spin axis on +Y. Terrain, chunks and the camera live in it, as in
  KSP's rotating frame near the surface.
- `SolarSystem` holds the star (real Sun mass and radius) and the planet's Keplerian orbit
  (1 AU, e = 0.0167), sidereal day (30 min) and axial tilt (23.44 degrees). Each frame the
  star's position is converted into the body frame (`star_in_body_frame`), and that alone drives
  day and night, the terminator seen from space, and the seasons. The star field is oriented
  with `planet_rotation`, so the stars turn across the night sky.
- Universal time `Game::time` advances by `dt * WARP_LEVELS[warp]`. Epoch 0 is the March
  equinox.
- When ships and other bodies arrive, this needs real reference-frame switching (inertial frame
  in space, rotating frame near the surface). Keep the f64 + camera-relative rules.

### Lighting and atmosphere (render/atmosphere.rs, shaders/)
- **HDR pipeline:**
  - Blocks render HDR radiance into `ColorTarget` (RGBA16F), in units where the sun's
    illuminance above the atmosphere is `SUN_ILLUMINANCE`.
  - `sky.frag` (a full-screen triangle) then reads the HDR colour and the depth buffer
    (`DEPTH_READ_ONLY_OPTIMAL`). For each pixel it ray-marches the atmosphere from the camera to
    the terrain or out to space, which gives sky, aerial perspective, sunsets and the limb glow.
    It also adds stars, the Milky Way, the sun's disk and glare, then applies exposure, ACES and
    dither into the swapchain.
  - Nothing else applies fog.
- **Atmosphere model:**
  - Earth's, scaled to the small planet: Rayleigh scale height 1.2 km at 10x Earth's
    coefficients, Mie 0.3 km, an ozone layer, and the top at 9 km.
  - Three LUTs are precomputed on the CPU at startup (about 25 ms): transmittance (Bruneton's
    parameterisation), multiple scattering (Hillaire 2020) and sky irradiance.
  - **`atmosphere.rs` and `atmosphere.glsl` must stay in sync** (LUT parameterisations, the
    medium, sizes).
- **Uniforms:**
  - One `FrameUniforms` UBO and one descriptor set per frame in flight (set 0, binding layout in
    `pipeline::BINDINGS`).
  - The push constants are now just mvp, offset and params.
- **Block lighting:**
  - sun × transmittance at the fragment (red at sunset, zero in the planet's shadow)
  - sky irradiance from the LUT
  - a ground bounce on walls and a night ambient
  - the fake cave sky factor and a faint headlamp
  - block light (torches): warm, `TORCH_IRRADIANCE` x Minecraft's brightness curve
    `l / (4 - 3l)`. The constant lives in `atmosphere.glsl` and app.rs (keep them equal).
- **Auto exposure** (`frame_params`):
  - It adapts partially (exponent 0.7) to the light at the camera. Above the atmosphere, only
    whether the camera itself is in sunlight counts.
  - Block light at the camera counts too (eye adaptation), so torch-lit spots at night and in
    caves aren't blown out.
  - Caves use an exposure up to 10x brighter than daytime (less near torches).
  - Stars get a boost above the atmosphere (artistic).
- **Stars:**
  - Procedural, at most one per cube-grid cell, and pixel-sized.
  - The expensive parts are skipped when the sky in front of them is bright.
  - The Milky Way is a precomputed equirectangular texture (`starfield.rs`).
- **Underground:** `FrameParams::sky = false` turns the atmosphere off (dark background plus
  cave fog).

### Block light (world/light.rs)
- Sources: torches (14), lit furnaces (13). The molten core blocks glow on their own faces
  (`Block::glow`) without spreading light.
- Each source keeps its own breadth-first flood fill (one level less per step, through
  non-opaque cells, across face edges and shell boundaries). `LightMap` holds the brightest
  level per lit cell and is shared (`Arc`) with the streaming workers.
- `WorldStreamer::set_light` adds or removes a source. `set_block` re-floods the sources whose
  light touches the changed cell. Both remesh exactly the chunks that show changed cells,
  including neighbours whose padding holds them.
- The mesher gives each face corner the average light of the open cells around it (smooth
  lighting, like its AO), in vertex bits 28-31. `LightMap::reaches` lets it skip lookups in
  chunks no source can reach.
- Sky light is still the mesher's estimate from depth below the column surface. Roofs don't
  darken what's under them, and digging a shaft doesn't let daylight in.

### Player and physics (player.rs, space/gravity.rs)
- **No physics engine for the player:** Keplerian gravity, two frames and curved-grid voxel
  collision don't fit one.
- **State:** the player's position and velocity live in the planet-centred inertial frame,
  where orbits live (`RotatingFrame` converts to and from the body frame). Each 1/120 s step:
  1. Controls change the velocity relative to the ground.
  2. Gravity is integrated in the inertial frame. `PlanetGravity` uses a layered density, the
     shell theorem inside, and 9.81 m/s^2 at sea level, so it's 0 at the centre. Air drag
     applies to the velocity relative to the co-rotating air.
  3. The player moves inertially and collides in the body frame (Minecraft-style per-axis AABB
     sweeps in grid (u, v, k) space). A blocked axis loses its velocity relative to the ground.
  - Standing still therefore means co-rotating (160 m/s at the equator). Centrifugal and
    Coriolis effects come out on their own, and jumping off something keeps its velocity.
- **Collision skin:** the box's size in grid units drifts slightly as the player moves
  (blocks differ in size), so sweeps use a `SKIN` of 1 mm.
  - A block overlapped by less than that counts as a contact and pushes the box back out.
  - A block overlapped by more is ignored, so you can walk out of a block placed inside you.
  - Without the skin, sliding along a wall leaked into it.
  - Blocked velocity is removed in grid components (`remove_blocked`), because u and v
    aren't perpendicular off the face centre lines.
- **Character assist (game feel):** applies only while walking and hopping, i.e. ground within
  `ASSIST_HEIGHT` (3 m) below and slower than `ASSIST_SPEED` (10 m/s).
  - Gravity pulls `CHARACTER_GRAVITY` (2.4x) harder, so a 1.25 m jump takes 0.65 s like
    Minecraft's, not the full second real gravity would take.
  - You get air control, and ease to a stop without input.
  - Long falls, launches, orbits and being thrown are pure physics.
  - Air control far from the ground needs thick air; there is none in vacuum.
- **Tests cover:**
  - standing, and a 1.25 m jump with its airtime
  - walking diagonally into walls and between diagonal pillars without entering them
  - a stable orbit, and rails vs. physics agreement
  - a fall through the planet's centre with no air
- **Time warp:**
  - Up to `MAX_PHYSICS_WARP` (x10) the steps simply run faster.
  - Beyond that, `can_rail_warp` must hold: landed, spectating, or in an orbit whose
    periapsis is above the atmosphere. Landed players stay put; orbiting players follow
    `Orbit::from_state(...).state_at(t)`.
  - Otherwise the effective warp is capped at x10 (the chosen warp is kept).
- **Unloaded chunks:** collision falls back to the terrain height (`WorldVoxels`), so you
  don't fall through ground that hasn't loaded yet.
- **Modes:**
  - Survival: physics, mining times and drops by tool, health, hunger and fall damage.
  - Creative: instant breaking without drops, placing doesn't use up items, a catalog
    screen, double-tap Space to fly.
  - Spectator: no gravity or collision. Movement uses real time, not warped time.
- **Fall damage** follows Minecraft: the height fallen from the highest point since last
  standing, minus 3 blocks, in half-hearts. Water and flight reset it. Armor doesn't help
  (as in Minecraft). Death drops everything carried, including armor, and you respawn at the
  spawn point.

### Survival gameplay (survival.rs, items.rs, ...)
- **Blocks vs. models:** `Block::is_opaque` means a full cube. `is_solid` adds the model
  blocks (`CraftingTable`, `Furnace`, `Chest`, `Torch`), which occupy their grid cell (for
  collision, targeting and edits) but are skipped by the mesher. `collides` excludes torches,
  which you walk through. `is_cutout` (glass) means see-through pixels. `Survival::objects` gives each one a box model
  (`models.rs`, 16 units = 1 block, Minecraft-style uvs), mapped trilinearly into the cell's
  curved and skewed shape. Facing (4 ways) turns the furnace door towards whoever placed it.
  Torches stand on a block or lean off its side (`ModelState::wall`), remember their support,
  and break with it. A chest's lid opens while its screen is open.
- **Mining:** `items::break_time` is Minecraft's formula:
  - hardness × 1.5 (or × 5 without the right harvest level) / tool speed
  - tool speeds: wood 2, stone 4, iron 6, diamond 8, gold 12
  - Each block has a preferred tool and a harvest level (stone and coal need any pickaxe,
    iron and copper need stone, gold and diamonds need iron).
  - Blocks drop only when harvestable (`items::drops`): stone gives cobblestone, ores give raw
    ores, coal or diamonds, and grass gives dirt.
  - Tools wear 1 per block (swords 2) and break at their tier's durability.
- **Combat:** swords and other tools have Minecraft's attack damage and attack speed
  (`Item::attack`, shown in tooltips). A hit deals damage x (0.2 + 0.8 charge^2), where charge
  is how far the weapon has recharged (the bar under the crosshair). Swords wear 1 per hit,
  other tools 2.
- **Animals** (`mobs.rs`): cows, pigs and sheep.
  - Spawning: in groups of 1-3 on grass, 20-56 m from the player, up to 10 around. They're
    forgotten beyond 128 m.
  - Behaviour: they wander, step up single blocks, and panic for 5 s when hit, with
    knockback and a red flash (0.5 s of invulnerability).
  - Loot: leather and beef, porkchops, wool and mutton.
  - Models are rebuilt every frame, since their legs animate, and they use the player's
    collision sweep. A ray against their box beats the block behind them for targeting.
- **Hunger** (Minecraft's rules): food 0-20 plus hidden saturation. Exhaustion costs a point
  of saturation, then food, per 4. It comes from sprinting 0.1/m, jumping 0.05 (sprint-jump
  0.2), hits 0.1, mining 0.005 and healing 6.
  - Regeneration: food 18+ heals 1 per 4 s; full food with saturation heals every 0.5 s.
  - Starving: food 0 hurts 1 per 4 s, down to 1 HP. Food 6 or less stops sprinting.
  - Eating: hold right click for 1.6 s. Cooked meat restores 8 (mutton 6), raw meat 2-3.
- **Armor:** 4 slots, Minecraft's points (leather 7, gold 11, iron 15, diamond 20) and
  damage formula (toughness 2 per diamond piece), with durability. Falls and starvation
  bypass it. Only attacks would use it, and nothing attacks yet.
- **Chests:** 27 slots (`Survival::chests`), spilled when broken.
- **Sound** (`audio.rs`): every sound is synthesized: footsteps per material every 1.6 m
  walked, mining hits, break and place, eat, burp, pickup pop, hurt, and the animals' calls
  and hurt sounds. 4 variants each, cached; volume falls off over 24 m and they are panned by
  direction. Survival code only pushes `(Sound, position)` events; the app plays them.
- **Drops** (`drops.rs`) fall and slide in the body frame, are picked up within 1.6 m (after
  a short delay when thrown), and despawn after 5 minutes. Blocks render as small cubes,
  items as crossed cutout sprites.
- **Furnaces** keep working when nobody watches, and under time warp (`Furnace::update`
  steps from event to event). Fuel: coal 80 s, logs and planks 15 s, sticks 5 s. Smelting
  takes 10 s per item.
- **Screens** are Minecraft's layouts (176 GUI pixels wide, 1 GUI pixel = `Ui::scale`), drawn
  by `screen.rs`. The click rules (`Survival::click`, `inventory::click`) and the crafting
  grid live in `Survival`. Closing a screen puts the grid and the held stack back into the
  inventory. Mouse drags (`Drag`) spread the held stack. Double-click gathers matching items.

### Camera
- The camera sits at the player's eyes (`EYE_HEIGHT` above the feet).
- "Up" is radial. `Camera::set_up` parallel-transports the frame for small changes. When up
  flips (more than 60 degrees in one step, e.g. flying through the planet's centre), it keeps
  the world-space view direction and re-derives yaw/pitch instead. Guard against normalizing a
  zero vector at the exact centre (`try_normalize`).

## Reference numbers (RTX 3060 Laptop, release, 2000x1125)
- With LOD to the horizon and the atmosphere pass: about 355 FPS uncapped at spawn (CPU about
  1.0-1.5 ms/frame with the player and HUD) and about 400 FPS on a mountain at 360 m. The atmosphere costs about 0.5-0.9 ms
  of GPU time (more sky means more ray-march steps). Night, with stars, runs at about 490 FPS.
- About 8k chunks are loaded at spawn, plus about 2.1-2.25k LOD tiles selected near the ground.
  Mesh VRAM is about 470-490 MB in total (chunks alone are about 325 MB). Dense jungle reaches
  about 690 MB because of the LOD trees, at about 9k quads per level-2 tile. Frustum culling
  skips about 65% of chunk meshes. From 40-90 km up, the whole planet takes 60-200 LOD tiles
  (8-25 MB).
- A full initial load takes under 4 s across 14 worker threads. Recomputing the desired set takes
  about 4 ms per chunk crossing.
- Deep underground (2.5-38 km), about 1.6-3.6k chunks are loaded, at 550-720 FPS, using
  roughly 60-200 MB of mesh VRAM (LOD is not drawn underground). A 5 km/s
  autofly straight through the core works, but streaming falls behind at that speed.
- The planet has 46,944 layers and 10 shells (surface radius 45.9 km).

## Known limitations / likely next steps
- LOD tiles are a heightfield (one terrain sample per cell), so overhangs and caves don't
  show at distance, and there can be a small visible step where real chunks take over. LOD jobs
  share the worker threads with chunk generation.
- Block light is real (torches), but sky light is still estimated from depth below the column
  surface. There are no terrain shadows (only the planet's own shadow) and no light shafts.
  Shadow maps or a sky-light flood fill would be the next lighting steps.
- The atmosphere is ray-marched per pixel at full resolution. Hillaire's sky-view and
  aerial-perspective LUTs would make it about 3x cheaper if needed. Exposure is a CPU estimate,
  not a luminance histogram. There is no moon yet.
- Survival is missing these Minecraft parts:
  - hostile mobs (so armor never gets used), shearing, breeding, saplings
  - lava
  - a player model in the inventory screen and a held item in first person
  - armor drawn on the player
- Water doesn't flow. Nothing is saved to disk: edits, inventory, furnaces and drops live in
  memory. Edited cells don't update the fake sky light, and LOD tiles don't show edits.
- The player is the only physics body. Rockets will need rigid bodies made of blocks, and
  KSP-style reference-frame switching once things fly far from the planet.
- The desired-set recompute runs on the main thread; it could move to a worker.
- Vulkan validation layers are only active if the LunarG SDK is installed. Install it before
  debugging rendering issues.
- Possible future work: real textures (PNG override of the generated ones), multi-draw indirect,
  occlusion culling for cave chunks.
