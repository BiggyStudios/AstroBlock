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
cargo test --release write_sounds -- --ignored --nocapture    # writes every sound effect to target/sounds/*.wav
cargo clippy --release --all-targets && cargo fmt               # keep both clean
```

`[profile.dev]` uses opt-level 1 for this crate and 3 for dependencies.
Shaders are GLSL in `shaders/`. `build.rs` compiles them to SPIR-V with **naga**, so no Vulkan SDK
or `glslc` is needed. A single source can produce several variants through defines (see the
`SHADERS` table in `build.rs`).

**Starting:** the game opens on the main menu (`ui/menu.rs`): worlds, settings, quit. Esc in a
world opens the pause menu (resume, settings, save and leave); F11 toggles fullscreen.

**Playing together:** "open to others" in the pause menu hosts the world (over Steam and on UDP/TCP
port 47777); "multiplayer" on the main menu joins by address or a Steam friend who is hosting.
Everyone warps on their own; "time" in the pause menu syncs to another player's time and weather,
follows their warp, or joins their team (a team warps as one).

**Controls:** click to capture the mouse. The ` key opens the debug console
(`help` lists the commands: `gamemode`, `tp`, `orbit`, `launch`, `time`, `warp`, `drag`, ...).
- **Moving:**
  - Survival / creative: WASD walks, Space jumps, Ctrl sprints. Double-tapping Space toggles
    flight in creative (Space/Shift to rise and sink).
  - Spectator: free fly through terrain. Space/Shift move along the radial "up", Ctrl gives
    10x speed, and the scroll wheel scales the fly speed.
- **Blocks and items:**
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
- **Time warp:** `.` and `,` raise and lower it (x1 up to x1,000,000), as in KSP. In a shared
  game it is your own (see `app/timeline.rs`).
- **Vehicles** (planes, rockets, rovers, built from blocks): right click a vehicle core on top of a
  block, then build onto it. R turns the next engine, wing or seat. Right click a seat to sit:
  the first one in flies it, and F stands up. Walk around on it like anywhere else.
  Flying: as for rockets, plus W/S drive wheels, A/D steer them, Space brakes, V sees from the
  seat. `vehicle <plane|rover|rocket>` in the console makes a ready-made one.
- **Rockets** (test rocket: `give rocket`, or `rocket` in the console):
  - Right click with it on top of a block stands it there; right click a rocket to get in,
    left click a landed one to pick it up.
  - Inside, the mouse is free (for the HUD's buttons): right-drag turns the camera, the wheel
    zooms. Shift / Ctrl throttle up / down, Z full, X cut. W/S pitch, A/D yaw, Q/E roll
    (yaw right tips it east on the pad). T toggles SAS; its modes are buttons above the
    attitude ball. F gets out, anywhere: in space you float beside the rocket on your own
    orbit (look around, but no moving); right click the rocket to get back in.
  - **M opens the map** (on foot too): right-drag and the wheel move its camera, Tab cycles
    the focus (craft, home, moon). Click your path to add a maneuver; its panel edits it.
    Delete removes the selected one.
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
| `ASTRO_COMMANDS="cmd;cmd"` | Console commands run 4 s after start, one per frame (e.g. `orbit 20`, `break`, `look`). For survival tests: `aim <pitch> [yaw]`, `give <item> [n]`, `select <1-9>`, `place` (= right click: places, opens, eats, wears), `throw`, `open inventory/creative`, `summon <cow|pig|sheep>`, `hurt <n>`, `heal`, `swing [secs]` (a slow-motion first-person swing, to capture its phases), `rain [n/s]` (test rain, GPU particles). Flight tests: `rocket [m]`, `board`, `exit`, `throttle <0-1>`, `fuel [percent]`, `steer <pitch> <yaw> <roll> <drive>` (holds those controls), `vehicle <plane|rover|rocket> [m]`, `sit`, `aboard [status]`, `cockpit`, `sas <mode|off>`, `orbit <km> [moon]` (in a rocket: puts it in orbit), `transfer` (plans a moon transfer node), `node <pro> [norm] [rad] [in s]`, `dive <m/s> [deg]` (reentry), `map`, `mapcam <yaw> <pitch> <km> [craft|home|moon]`, `chase <yaw> <pitch> <m>`, `lookat <moon|home|sun|rocket>`, `goto <moon|home> [lat lon]` (stand on that world's ground; on foot). Weather tests: `weather` (what the sky is doing here), `weather <clear|fair|cloudy|fog|drizzle|rain|storm|snow|blizzard|sandstorm|tornado>` (holds that within 9 km until `weather auto`), `weather strike` (a bolt straight ahead), `weather approach [m]` (go to that far from the nearest tornado), `erupt` (the nearest volcano; prints where it is), `lookat <tornado|volcano>`. Multiplayer tests: `net host 127.0.0.1:<port>` (**bind to 127.0.0.1 in tests**: any other address raises a Windows Firewall prompt), `net join 127.0.0.1:<port>`, `net status`, `net leave`, `net nudge <m>` (stand that far ahead), `net hit` (hit the nearest animal someone else simulates), `net sync <who>`, `net follow <who>`, `net unfollow`, `net team <who>`, `net team leave` (who: player number or name), `lookat player`, `wait <s>` (holds up the rest of the command list). Run two captures at once, the client started later. The worker needs a moment: put a few dozen no-op commands (`heal`) between `weather tornado` and `lookat tornado`. Captures are silent (no audio device is opened). |
| `ASTRO_MENU=1` | Start on the main menu even in a capture (otherwise captures and runs with `ASTRO_SPAWN`, `ASTRO_MODE`, `ASTRO_TIME` or `ASTRO_COMMANDS` go straight into an unsaved world). Menu tests: `menu <main\|worlds\|create\|settings\|pause>`, `world create <name> [seed] [mode]`, `world load <name>`, `world leave`, `save`, `settings [vsync on\|off] [fps <n>] [volume <0-100>] [sensitivity <25-400>] [smoothing on\|off]`. |
| `ASTRO_SAVE_DIR=<folder>` | Where worlds and settings are kept. **Automated tests must set this**: a capture without it can't save at all, so it can't touch the owner's worlds. |
| `ASTRO_STEAM=1` | Use Steam even in a capture. Off by default there: a test run would show the owner as playing on their account. |
| `ASTRO_NAME=<name>` | The name others see (tells test instances apart). |
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
  main.rs           the module list; starts the winit event loop
  audio.rs          sound effects synthesized in code; a small mixer on a cpal stream, panned by position
  util.rs           FxHash FastMap/FastSet (use these for hot hash maps keyed by ChunkKey etc.)
  app/              the game itself: `Game` and what ties the other systems together
    mod.rs          winit event loop, Game (player, time + warp, sun, exposure, screens, console commands), `build_world`
    input.rs        per-frame key/mouse state
    save.rs         what is on disk: settings, world list, atomic file writes
    session.rs      creating, saving, loading and leaving worlds, the loading screen, the menu backdrop, applying settings, the frame cap
    flight.rs       the game's flight side: rockets, boarding, controls, chase and map cameras, predictions, effects, flight console commands
    weather.rs      the weather around the player: particles, lightning and thunder, tornado physics, flying blocks, eruptions, the roof map, ambient sound, console commands
    lava.rs         rivers of lava, lava bombs, lava running into openings, cooling
    worlds.rs       the two block worlds (home, the moon): which is active, switching, the other one's far terrain
    multiplayer.rs  hosting and joining; every message's effect on the game; other players' figures and name tags
    timeline.rs     each player's own clock and warp: sync, follow, teams, riding along, whose weather you watch, rockets at another time
    vehicles.rs     vehicles in the game: starting and building them, walking aboard, seats, passengers
    death.rs        dying: the death screen, watching the wreck, respawning
  player/
    mod.rs          Player: inertial-frame physics, voxel collision, modes, rails warp, fall height
    interact.rs     ray cast to the targeted block, its outline matrix
    avatar.rs       how other players look (a box-model figure in a space suit)
    viewmodel.rs    first-person arm + held item: pose and animations (swing, use, switch, eat, bob, sway, landing)
  survival/
    mod.rs          Survival: inventory, armor, held item, mining/placing, torches, drops, models, furnaces, chests,
                    mobs (spawning, combat), screens' click rules (drag, double-click), health, hunger, sound events
    items.rs        Item, ItemStack, tools (kind x tier), Minecraft mining rules, block drops, fuel, smelting
    inventory.rs    36-slot Inventory, add/sort, Minecraft slot clicks
    crafting.rs     shaped recipes (mirroring, "any planks")
    furnace.rs      Furnace: fuel and smelting, steps from event to event (works under time warp)
    drops.rs        items lying in the world: physics, pickup radius, despawn
  mobs/
    mod.rs          passive animals (cow, pig, sheep): wander / graze / panic AI, physics, hits, loot
    models.rs       the animals' box models (many small tinted boxes, tiled coats) and their animation
  flight/
    vessel.rs       Vessel: the test rocket's physics (thrust, reaction wheels, SAS, drag, fins, reentry heat, landing, crashing, SOI changes, rails)
    craft.rs        Craft: a vehicle's grid of parts, and what follows from them (mass, centre of mass, inertia, engines, wings, wheels)
    craft_physics.rs  a vehicle as one rigid body: engines and wings where they sit, wheels on springs, the hull against the ground
    craft_model.rs  a vehicle's mesh (rebuilt when its parts change, drawn turned into place)
    rocket_model.rs the rocket's box model (glows when hot)
    hud.rs          flight HUD: attitude ball with direction markers, throttle and heat gauges, SAS buttons, telemetry; `Pointer` + `button` widgets
    map_view.rs     map overlay: patched-conic trajectories, markers, the maneuver planner (`Planner`) and its node panel
  effects/
    mod.rs          particle effects: block debris, mining chips, placing dust, the test rain, rocket flame / smoke / plasma / explosions, weather (rain, snow, sand, ash, tornado dust, lightning, eruptions) (emitters for render::particles)
    debris.rs       whole blocks flying through the air (tornadoes, lava bombs): physics, cached cube meshes
  ui/
    mod.rs          immediate-mode quads, outlined text, flat block tiles, sprites, fading/shifting quad groups
    hud.rs          overlay in the minimal flat style: telemetry text, vitals as number + line, hotbar row, console
    menu.rs         main, world list, new world, settings and pause menus (immediate-mode, reports `Action`s)
    screen.rs       inventory (suit + assemble) / workbench / furnace / storage / catalog screens: layout, drawing, details card
    console.rs      debug console (line editor, history, help text)
    anim.rs         UI motion helpers: approach / ease_out, item pops, vitals with damage trails
  net/
    mod.rs          `Link`: the open connections, over either transport; `Event`, `PeerId`
    protocol.rs     every message that crosses the network (`Msg`), postcard encoding, deflate
    direct.rs       IP connections: TCP for what must arrive, UDP for the rest
    steam.rs        Steam peer-to-peer (as app 480, Spacewar), friends, invites
  weather/          the planet-wide weather: `sim` (fields on a cube-sphere grid, on its own thread), `mod` (`Weather`, `Snapshot`, `Sample`, `Condition`)
  space/
    mod.rs          SolarSystem (star, planet orbit/spin/tilt), RotatingFrame (body <-> inertial), local time
    orbit.rs        Keplerian orbits: elements <-> state vectors (Kepler's equation)
    bodies.rs       Body (sun, home, moon), Bodies: gravity, SOIs, positions, rotating frames (the moon is tidally locked)
    conic.rs        universal-variable Kepler propagation (any conic), Conic geometry (apsides, anomalies, times)
    trajectory.rs   patched-conic prediction (SOI encounters, exits, impacts, maneuver nodes) and rails propagation
    gravity.rs      PlanetGravity: layered-density gravity inside and out, air density
  render/
    mod.rs          Renderer: frames in flight, uploads, culling, scene passes, atmosphere pass, overlay, screenshots
    camera.rs       f64 position + planet-relative "up" frame (handles flips at the centre), reverse-Z projection (+ the view model's squeezed-depth variant)
    models.rs       box models (crafting table, furnace, chest, torch, dropped items, the first-person arm and held item) -> ChunkMesh
    atmosphere.rs   Atmosphere model + CPU-precomputed LUTs (transmittance, multiple scattering, sky irradiance)
    starfield.rs    Milky Way map (precomputed equirectangular texture)
    font.rs         Astro Block's own pixel font: condensed, square-cornered, 4x8 glyphs in 5x9 cells
    context.rs      instance/device (Vulkan 1.3: dynamic rendering + synchronization2), submit_and_wait
    swapchain.rs    swapchain, D32 reverse-Z depth buffer (sampled), HDR colour target (RGBA16F)
    pipeline.rs     opaque / water / far (LOD + fading chunks) / sky (full-screen) / ui pipelines, one layout
    arena.rs        VertexArena (mesh memory in 64 MiB pages, power-of-two blocks) + StagingRing
    particles.rs    GPU particles: ring buffer, emitters, anchor re-basing, per-frame batch for the compute shaders
    cloud_noise.rs  tiling 3D noise volumes clouds are carved from (generated at startup)
    mesh.rs         BlockVertex (12 bytes), UiVertex, PushConstants (112 bytes: + a rotation), ChunkMesh, DrawItem, Fade
    texture.rs      block texture array with CPU-generated mips; single-mip 2D textures (LUTs, font)
    frustum.rs      frustum planes for camera-relative bounding spheres
  world/
    planet.rs       solid cube-sphere: Face, Shell, Planet, cells (cell/cell_at/cell_chunk), wrap_column, chunk_neighbors
    chunk.rs        ChunkKey, BlockCell, ChunkBlocks (Uniform | Dense), PaddedBlocks (34^3)
    edits.rs        WorldEdits: player edits, applied on top of generation (chunk + padding)
    light.rs        block light: per-source flood fills, shared LightMap read by the meshing workers
    terrain.rs      TerrainGenerator: columns/climate/biomes, caves, ores, trees, column cache, deep interior; the moon's lunar mode
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
  ui.vert/.frag     overlay: font atlas, block tiles or item sprites, alpha blended over the final image
  weather.glsl      shared: weather maps, cloud noise, what clouds do to sunlight (`cloud_light`)
  clouds.frag       half resolution: volumetric clouds, the low layer (rain curtains, fog, dust, blizzards, ash), tornado funnels, northern lights
  clouds_resolve.frag   temporal accumulation of clouds.frag
  particle.glsl, particle_batch.glsl   particle struct + flags; the compute shaders' per-frame batch (header + emitters)
  particles_emit.comp, particles_sim.comp   spawn new particles; gravity, drag, depth-buffer collision
  particles.vert/.frag   camera-facing quads or velocity streaks, lit like blocks (variant BLEND)
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
   - **Shells:** columns are mapped onto the sphere by the face map (see "Block shape"), and the grid
     resolution depends on depth (`Planet::shells`). The surface shell has
     `face_blocks = chunks_per_face * 32` columns per face edge. Each deeper shell halves the
     resolution (2x2 columns become 1) where blocks have shrunk to about 0.5 m, so widths stay
     about 0.5-1 m down to the small innermost shell, which tapers to the centre.
   - **Shell boundaries** are multiples of 32 layers, so a chunk belongs to exactly one shell.
     Always get the resolution from `res_at(k)`, and pass `n` explicitly to `dir`,
     `corner_dir` and `wrap_column`. `column_dir` means
     surface columns. `locate` returns `u` and `v` in the grid units of the shell at that
     radius.
   - **Neighbours across a shell boundary:** a coarse chunk has 4 finer chunks above it, and a
     fine chunk has 1 coarse chunk below it (`chunk_neighbors`). A coarse grid's corners
     coincide bit-exactly with every other corner of the finer grid above it.
   - **Handedness:** the `(u, v, k)` frame is right-handed (`s x t = normal`).
   - **Face edges:** neighbours across face edges have rotated grids, so always go through
     `wrap_column` / `chunk_neighbors`. The face map is evaluated from |x| and y^2 (mirrored
     coordinates give bit-identical results) and keeps A(+-1, y) = +-1 exactly. Edge points
     use `edge_to_cube` on both faces. `corner_dir` is exact at edges, which makes shared
     vertices bit-identical across faces.
   - **Block shape:** blocks are always 1 m tall, but no grid can cover a sphere with equal
     squares. At the 8 cube corners only 3 columns meet, so the grid has to bend somewhere.
     - **The face map** (`Planet::face_to_cube` / `cube_to_face`, coefficients `MAP_Q`):
       face coordinates (x, y) become cube coordinates (A(x, y), A(y, x)), with
       A = x (1 + (1 - x^2) Q(x^2, y^2)).
       - Q was fitted by least squares to be nearly *conformal* (square blocks), except
         within 0.3 face half-widths of a corner, with a limited spread of sizes.
       - The inverse is a Newton iteration inside the face and exact outside it.
       - Past a face edge the map continues in angle (atan(a) linear in x), so far-away grid
         coordinates stay bounded, as the old tan map's did. Without that, collision sweeps
         through far grid space become enormous (a test took 10 minutes).
     - **Results:** 92% of surface blocks are within 1 degree of square (96% within 3).
       Widths are within 10% of the median for 80% of blocks and within 20% for 91%, from
       1.08x the median at face centres down to about 0.5x at the corners.
     - **Median width:** `BLOCK_WIDTH` = 1 m, so the home planet (65,536 columns per face
       edge) has a 43.9 km radius. Widths run from 0.5 to 1.08 m. `find_spawn` looks for
       blocks within a few percent of 1 m squares. `Planet::new` derives the radius from
       `BLOCK_WIDTH` and `MAP_MEDIAN_SCALE`.
     - **Valid sizes:** every shell's resolution must stay divisible by 32. With 1 m blocks
       that allows 1024, 1536, 2048, 3072, 4096 or 6144 chunks per face edge (22.0, 32.9,
       43.9, 65.9, 87.9 or 131.8 km), then doubling from there.
     - **Before:** the separable tan(38 deg) map made grid lines great circles. Only 28% of
       blocks were within 3 degrees of square, and 9% were 20-30 degrees off.
     - **Corner seas** (`TerrainGenerator::corner_sea`): the distorted 5% of the surface lies
       under round, deep seas, full within 11 degrees of a corner with the coast by about 15
       degrees, ragged. Distorted means skew > 5 degrees or width off the median by > 20%; all of it is
       within 10 degrees of a corner.
     - `find_spawn` picks open land (no tree within 3 columns) where blocks are within a few
       percent of 1 m squares.
     - The fitting tool isn't in the repo. To refit: least squares (Levenberg-Marquardt) of
       skew, log aspect ratio and log size spread over a face quadrant, with Q of degree 4.
     - Tests: `face_map_is_exact_symmetric_and_invertible`, `cube_to_face_converges_quickly`,
       `surface_blocks_are_nearly_cubes`, `distorted_blocks_are_under_the_corner_seas`,
       `spawn_blocks_are_square`.
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
- Push constant `params.z < 0` marks the first-person view model (no target outline).
- Push constant `rotation` (quaternion) turns a mesh about its origin (`DrawItem::rotation`);
  identity for everything but the other world's terrain (home seen from the moon, or the
  moon from home), drawn turned into the active world's frame.
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
  8-15 a brightness (0 = full), and bit 31 marks a sprite (`ui/mod.rs` and `ui.frag`
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
  (From the *focus*, really: the camera on foot, the rocket when flying. The LOD tiles are
  chosen for the render camera, which differs in the map view.)
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
  After the models (step 2), rendering pauses for the particle compute work (simulate +
  spawn against the world's depth), then resumes. Cut-out particles come next, blended
  particles after the water (step 4).
  5. the first-person view model (clip pipeline, `Camera::view_model_projection`: the same
     picture with depth squeezed into 0.6..1, so it is always in front of the world, which
     is never nearer than 8 cm, and never pokes into walls)

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
- Flight uses patched conics around the sun, home and the moon (`space::bodies`, see "Flight").
  Everything is still drawn in the home planet's body frame, converted at the current time.
  Keep the f64 + camera-relative rules.

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
    `l / (4 - 3l)`. The constant lives in `atmosphere.glsl` and app/mod.rs (keep them equal).
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

### Player and physics (player/mod.rs, space/gravity.rs)
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

### Survival gameplay (survival/, mobs/)
- **Blocks vs. models:** `Block::is_opaque` means a full cube. `is_solid` adds the model
  blocks (`CraftingTable`, `Furnace`, `Chest`, `Torch`), which occupy their grid cell (for
  collision, targeting and edits) but are skipped by the mesher. `collides` excludes torches,
  which you walk through. `is_cutout` (glass) means see-through pixels. `Survival::objects` gives each one a box model
  (`render/models.rs`, 16 units = 1 block, Minecraft-style uvs), mapped trilinearly into the cell's
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
- **Animals** (`mobs/mod.rs`): cows, pigs and sheep.
  - Spawning: in groups of 1-3 on grass, 20-56 m from the player, up to 10 around. They're
    forgotten beyond 128 m.
  - Behaviour: they wander, graze while standing, step up single blocks, and panic for 5 s
    when hit, with a red flash (0.5 s of invulnerability).
  - They fall at `WEIGHT` (2x) gravity, like the player's hops. A hit *replaces* their
    velocity with a short hop away (`KNOCKBACK` 3.2 m/s, lift 3 m/s): about 1.3 m in all.
    (It used to add 5 + 4 m/s under plain gravity, and they flew several metres.)
  - Loot: leather and beef, porkchops, wool and mutton.
  - Models (`mobs/models.rs`) are the game's own, sculpted from many small boxes: barrel
    body with ridge, belly, chest and rump; a neck; a head with muzzle or snout, eyes, ears
    (horns on cows); a tail; legs with hooves. Coats are near-white seamless textures
    (`Tex::Fur`, `Patches`, `Wool`, `Hide`) tiled at block texel density and coloured per
    part by vertex tints (`Quad::tint`), with 4 coat variants per animal. Nothing is stretched.
  - Motion (`MobPose`): legs swing in diagonal pairs, body bob and head nod with the walk;
    standing animals lower their heads to graze (the neck stretches down), look around,
    swish their tails and flick their ears.
  - Models are rebuilt every frame and they use the player's collision sweep. A ray
    against their box (longer in front, where the head is: `MobKind::length`) beats the
    block behind them for targeting.
- **Death** (`app/death.rs`): `Game::die` drops the items (survival only) and the world carries
  on until the death screen's button (or `respawn` in the console) calls `Game::respawn`. In a
  crash the camera circles the wreck; otherwise the view sinks to the ground. A crash stops at the
  screen in creative too. While dead, `Survival::dead` keeps the player from being hurt or picking
  up what they dropped, and others see nobody. Dead worlds aren't autosaved, and leaving respawns first.
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
  walked, mining hits (`Dig`), break and place, eat, burp, pickup pop, putting on armor
  (`Equip`), hurt, and the animals' calls and hurt sounds. 4 takes each at slightly
  different pitches, cached; volume falls off over 24 m, panning is limited to 80%, and the
  mix is soft-limited (tanh). Survival code only pushes `(Sound, position)` events; the app
  plays them.
  - Kept low and soft (the first version was bright filtered noise, 2-5 kHz, and sounded
    thin and high-pitched). Building blocks: `thump` (a pitch-dropping sine, the weight of an
    impact; 95-170 Hz so laptop speakers still play it), `ring` (decaying modes: stone, wood,
    glass), `rustle` (band-passed noise in grains: crunch, rustle) and `voiced` (a Rosenberg
    glottal pulse through three vowel formants, with breath and a subharmonic growl: moo,
    grunts, bleat, the player's "oof").
  - Per material, `impact(strength)` mixes these; a step is light and dull, a dig medium,
    placing heavy, and breaking adds a few smaller pieces settling (and tinkles for glass).
  - The test `sounds_are_short_soft_and_in_range` checks every take's length, peak and
    brightness (RMS frequency under 1.6 kHz, glass 3 kHz).
    `cargo test --release write_sounds -- --ignored --nocapture` writes them all as WAV
    files into `target/sounds` to listen to.
- **Drops** (`survival/drops.rs`) fall and slide in the body frame, are picked up within 1.6 m (after
  a short delay when thrown), and despawn after 5 minutes. Blocks render as small cubes,
  items as crossed cutout sprites.
- **Furnaces** keep working when nobody watches, and under time warp (`Furnace::update`
  steps from event to event). Fuel: coal 80 s, logs and planks 15 s, sticks 5 s. Smelting
  takes 10 s per item.
- **Screens** (`ui/screen.rs`) use the game's own minimal flat style: no panel, the world dims
  behind a grid of faintly outlined cells (20 GUI-pixel pitch, 1 GUI pixel = `Ui::scale`),
  with small lowercase section labels ("suit", "assemble", "backpack", ...), thin progress
  lines and an amber outline on result cells. The click rules (`Survival::click`, `inventory::click`) and the crafting
  grid live in `Survival`. Closing a screen puts the grid and the held stack back into the
  inventory. Mouse drags (`Drag`) spread the held stack. Double-click gathers matching items.


### Look and feel (UI)
- Astro Block's own **minimal flat** style, chosen by the owner. It must not look like
  Minecraft (see the top of this file).
  - No panels or bevels; text is lowercase in the game's own condensed pixel font, with a
    soft outline (no drop shadow).
  - Crosshair: a dot. Mining, eating and recharging show as a short line under it.
  - Vitals: a number (0-100) plus a thin line, health on the left and food on the right
    above the hotbar. Armor is a faint second line. They turn red when low.
  - Hotbar: items in a quiet row, a dot for empty slots, an underline under the selection.
  - Blocks show as flat tiles in the UI, not isometric cubes. The targeted block gets a soft
    light edge, not a dark outline.
  - Palette: off-white text, dim blue-grey labels, an amber accent for results and food.
- **Motion:** the UI is immediate-mode, so animation state lives in `hud::HudAnim` and
  `screen::ScreenAnim` (owned by `Game`), built on `ui/anim.rs`.
  - Helpers: `approach` (exponential, frame-rate independent), `ease_out`, `Pop` (a slot
    grows briefly when it *gains* items) and `Vital` (eases, with a held-then-draining trail
    after a loss).
  - Groups of quads are animated after drawing them normally: `Ui::fade_from(start, alpha,
    dx, dy)` scales alpha and shifts everything since vertex `start`. Shifts snap to whole
    pixels so the font stays crisp.
  - HUD:
    - The hotbar underline slides between slots. The selected item lifts and the others dim.
    - Pickups pop.
    - Vitals count and ease. Damage leaves a red trail, flashes and shakes the health number,
      and flashes the screen edges red. Healing brightens it. Low health pulses.
    - On a target, four corner ticks close in around the dot. The progress line fades in and
      out.
    - Item names, messages and the console fade and slide.
  - Screens:
    - Opening fades the dim in and lifts the content into place (0.18 s).
    - Closing replays the last frame's vertices fading and sinking (0.12 s).
    - Hover glows trail behind the mouse, and the details card fades in.
    - Slots pop when items land in them, and a ready result breathes.
  - Everything runs on real frame time, never warped time.

### Particles (render/particles.rs, effects/mod.rs, shaders/particle*)
- **GPU particles.** A ring buffer of `CAPACITY` (262,144) particles, 64 bytes each, lives
  on the GPU. Compute shaders spawn and simulate them, so weather (rain, snow, sand, storm
  debris) can use hundreds of thousands without CPU cost.
  - **Spawning:** the CPU only sends `Emitter` requests (`Renderer::emit_particles`): a count,
    a spawn box, velocity (base + outward burst + random spread), life and size ranges,
    gravity scale, drag towards the wind, bounciness, a texture rect (or a random
    `piece` of it), a tint, flags and light. `particles_emit.comp` writes the new particles
    after the newest ones, overwriting the oldest when the ring is full. Up to 256 emitters
    and 65,536 new particles per frame.
  - **Simulation** (`particles_sim.comp`, with the spawning) runs in the middle of the scene
    pass (`Renderer::particle_compute`; rendering pauses and resumes with load ops). It runs
    after the world (chunks, LOD, models, mobs, drops) and before particles, water and the
    arm, so particles collide with the world only. They used to collide with their own
    quads, and debris climbed towards the camera. Particles are drawn the same frame:
    - radial gravity (`FrameParams::gravity`) and drag towards `FrameParams::wind`
    - collision with the finished depth buffer, only when a particle *crosses* a visible
      surface: it was in front of what the camera saw at its spot and is now just behind it.
      A particle that is already hidden keeps falling. Pushing hidden particles onto the
      surface in front of them threw debris upwards: it spawns inside the broken block,
      whose mesh is still drawn for a frame or two, and falls behind the rim of the hole.
      The normal comes from neighbouring pixels.
      `BOUNCE` particles bounce and then settle (`RESTING`, and they stay while something is
      under them). `DIE_ON_HIT` particles vanish.
    - Particles off screen don't collide. That is fine for debris and weather; anything
      needing exact block collision would be done on the CPU.
  - **Drawing:** 6 vertices per ring slot of the live range, with no vertex buffer; dead
    particles collapse.
    - Cut-out particles (alpha test, depth write) are drawn with the scene. `BLEND` particles
      are drawn after the water without writing depth.
    - `STRETCH` makes a streak along the velocity (rain), `ROUND` a soft disc, `TINT_MASK`
      reads a block texture's alpha as the biome tint mask.
    - `GLOW` particles light themselves (rocket flames, reentry plasma, explosions): their
      colour times the sun's illuminance, unaffected by sun or shadow.
    - Lighting is like the blocks (sun, sky, night, torches) from the emitter's light.
    - Cut-out particles shrink away at the end of their life, blended ones fade.
    - Everything shrinks away within a metre of the eye.
  - **Precision:** positions are f32 relative to an *anchor* near the camera. Past 256 m the
    anchor moves to the camera and the next simulation step shifts every particle
    (`shift`). Draws use the anchor the buffer is in.
  - **Live range:** the CPU tracks spawned ranges and their latest expiry
    (`ParticleFrame`), so draws and simulation cover only slots that may be alive.
  - **Time step:** game time, capped at x10 warp and 0.1 s.
- **Effects** (`effects/mod.rs`), kept deliberately gentle (the owner found a burst towards the
  camera bad):
  - On breaking: 24 small crumbs of the block's side texture, starting inside its cell,
    popping up a little and falling.
  - While mining: four chips per tick, hopping a little off the face and falling.
  - On placing: a little dust.
  - Nothing should fly at the player.
  - Survival queues them in `Survival::effects`. The app lights each one with the block and
    sky light at its spot and sends them on.
  - `rain [n/s]` is a test shower around the camera (blended streaks that die on impact),
    not the weather system.
- **Cost:** about 0.2 ms for roughly 85k live rain drops, 0.4 ms for a full ring
  (RTX 3060 Laptop).

### Flight (flight/vessel.rs, space/, app/flight.rs, flight/hud.rs, flight/map_view.rs)
- **Bodies** (`space::bodies`): the sun, home (43.9 km, 9.81 m/s^2, atmosphere to 9 km)
  and the moon: an 11 km block world, 1.63 m/s^2, 880 km out (20 home radii, like KSP's Mun), in
  home's equatorial plane, tidally locked (its +X faces home). SOIs by Laplace's formula:
  moon about 150 km, home about 17,000 km. Low home orbit is about 520 m/s, and the moon
  transfer from 30 km is about 180 m/s.
- **Patched conics:** a craft's state is relative to its reference body (`Vessel::body`) in
  shared inertial axes; only that body pulls. Crossing an SOI boundary re-expresses it
  (`Vessel::change_soi`, `trajectory::transfer`).
  - `conic::propagate` is universal-variable Kepler (ellipses and hyperbolas alike,
    Laguerre-Conway); `Conic` gives apsides, anomalies and times.
  - `trajectory::predict` chains segments: SOI entries are found by sampling (400 per
    segment) and bisection; surface impacts and SOI exits exactly from the conic's radius
    crossings; maneuver nodes split segments and apply their burn. About 1.5 ms/frame with
    the map open.
  - `trajectory::advance` moves a craft on rails during time warp and stops where physics
    has to take over (entering an atmosphere, `RAILS_CLEARANCE` above an airless surface).
- **The rocket** (`flight/vessel.rs`): 3 t full (2.2 t empty), 37 kN (TWR 1.26 at home), 8 m
  tall. Axes: +Y nose, +X right, +Z dorsal; on the pad right points east.
  - Fuel: 800 kg worth 1,890 m/s (`Vessel::delta_v`, the rocket equation); it lightens as it
    burns, the engine stops when it is gone, and it refills only landed on home (20 s). A
    decent ascent uses about 1,200 and takes about 75 s of burning, so the moon and back
    (about 600) leaves little to spare. Change `DRY_MASS`, `FUEL_CAPACITY`, `THRUST` and
    `EXHAUST_SPEED` together; `the_tank_runs_dry` and the launch test pin the budget.
  - Steering by reaction wheels; SAS holds an attitude or turns towards prograde /
    retrograde / normal / antinormal / radial / the next maneuver. Low down (home < 12 km,
    moon < 4 km) these are relative to the surface.
  - Drag on the velocity relative to the turning air, far larger side-on; fins weathervane
    the nose into the airflow.
  - Reentry heat: flux 0.05 x sqrt(air density) x speed^3 above ~300 m/s (gameplay-tuned:
    speeds here are a tenth of Earth's), radiative and convective cooling. Burns up at
    1,700 K. Steep returns at 950 m/s burn up, shallow ones and 850 m/s returns don't.
  - Ground: the player's voxel sweep (an upright 2 x 4 m box at the engine) against
    whichever block world is loaded (`vessel::Ground`). Faster than 12 m/s is a crash;
    settling more than ~50 degrees off vertical tips it over. Landed, it stands upright and
    turns with the body.
  - Tests: pad then gravity turn to orbit, reentry heating, landing on the moon.
- **Game side** (`app/flight.rs`): `Flight` holds the rockets, the boarded one, the cameras,
  the `Planner` (maneuver nodes) and the prediction.
  - While boarded, the player's physics is off and it rides along; streaming loads chunks
    around the rocket (`stream_focus`). The LOD terrain is chosen for the render camera
    (`WorldStreamer::update(focus, lod_camera)`), so the map sees the whole planet.
  - Chase camera: around the rocket in its local horizon frame. Map camera: around the
    focus in *inertial* axes (the planet turns under the orbit, as in KSP).
  - Maneuver nodes: (prograde, normal, radial) dv at a time. While burning near a node,
    the dv the engine delivers (`Vessel::burned`) is subtracted in the node's frame, so the
    planned path stays put; it completes below 0.1 m/s or on overshoot. "Warp to" picks
    the fastest warp that won't overshoot.
  - Getting out (`Eva` in app/flight.rs): landed on home you stand beside the rocket.
    Anywhere else you float 4 m to its right with its velocity, following your conic
    exactly like the rocket (patched conics, rails, SOI changes, `trajectory::advance`), so
    the two stay together. Coming down into home's air, or near the moon's blocks, hands
    over to the player's own physics (walking); on foot above them (`launch`, or a big
    jump off the moon) you start floating. Telemetry is then relative to the body you float around.
  - Empty rockets landed ride the turning ground; others coast on rails, with physics
    only in the air (they can crash unattended).
  - Chase camera drags: right moves the view right (orbits like the map).
  - Effects: exhaust flame (glowing) and smoke in air, reentry plasma, explosions (fire,
    smoke, hull debris); an engine rumble replayed while burning.
- **HUD** (`flight/hud.rs`): an attitude ball (sky / ground, horizon, pitch rings, compass
  letters, the direction markers), fuel, throttle and hull temperature lines, SAS buttons,
  telemetry, the next burn's countdown. **Map overlay** (`flight/map_view.rs`): paths, apsides,
  encounters, impacts, body names and rings, the moon's orbit, nodes and their panel.
- **Rendering:** rockets are rebuilt every frame (`rocket_model`).

### Vehicles built from blocks (flight/craft*, app/vehicles.rs)
- **A vehicle is a `Vessel` with a `Craft`**, so the map, time warp, SOI changes, saving and the
  network come for free. `Vessel::position` stays the centre of mass; it moves when parts or
  fuel change, so every change goes through `rebuild_craft`, which keeps the parts where they
  are in the world. Grid coordinates are meters from the core, +Y up where the core was placed.
- **Parts** are `Block`s: any ordinary block (by weight: `part_mass`), plus `is_vehicle_part`
  ones, which only exist on a vehicle. `Part::facing` is a direction index: where an exhaust
  leaves, which way a wing lifts. `auto_facing` picks it from where the part is put, R overrides.
- **`Stats`** is everything that follows from the parts (`rebuild`). `forward` is where the
  engines push, else where the core faces; the pilot's pitch, yaw and roll are about those axes.
- **Physics** (`craft_physics`, 240 Hz): forces at each part, in grid axes, so position decides
  the turning. Engines have their own relation to air (`AirUse`). Each surface finds its own
  angle to the flow and stalls by itself; flaps deflect to give the torque asked for. Body drag
  uses the silhouette along each axis. Reaction wheels: the core and `Gyro`s. Flaps stop at the
  stall angle, and are solved together (`FlapPlan`) so they give the turn asked for and no other:
  a fin asked to help a roll would otherwise swing the nose. Planes need flaps out on the wings
  to roll well (the ready-made plane's outer wing blocks). The stall guard (`guard_the_stall`) keeps the pilot and SAS from pitching fixed
  wings past it, so holding the stick back rides the edge of the stall; tests pin this.
  Numbers to tune: `engine_kind`, `part_mass`, `TANK_FUEL`, the constants at the top of `craft_physics`.
  Atmosphere is thin high up (1.2 km scale height), so planes top out at a few km.
- **Ground:** wheels are spring-damper rays along grid -Y that see `STEP_UP` above the axle, so
  they climb single blocks (the terrain is all terraces). The hull meets the ground at its outer
  corners with impulses. Hitting faster than `CRASH_SPEED` breaks that part off (and whatever it
  held on: anything no longer joined to the core comes off), `WRECK_SPEED` or losing the core
  destroys it. Still for `STILL_TIME`, it becomes `landed` as a rocket does.
- **Aboard** (`Flight::aboard`): the player is held in grid coordinates and moves there, down
  is always the deck's -Y whatever the vehicle does, and the view turns with it. On foot, vehicles
  are solid (`meet_vehicles`, a sweep in grid coordinates); landing on one puts you aboard, and
  walking off the edge hands you back to the world's physics with the vehicle's speed.
  `meet_vehicles` must run after `update_vessels`: a parked vehicle's inertial position is only
  right at the time it was last synced, and the planet turns it over a metre per frame.
  For the same reason walking off only takes effect at the end of `place_aboard`. Taking off in
  creative leaves the vehicle (`toggle_flight`), and a flying player never lands on one: to land,
  stop flying.
- **Seats:** the first to sit flies it (`boarded`, as a rocket); anyone else aboard rides along
  and their clock is attached to the pilot's (`keep_ride`).

### Two worlds: home and the moon (app/worlds.rs)
- **The moon is a block world** like home: its own `Planet` grid (512 chunks per face edge,
  16,384 columns, an 11 km datum radius, 512 layers above it), `TerrainGenerator::moon`,
  `WorldStreamer` (chunks + LOD) and `PlanetGravity::airless` (1.63 m/s^2, no air, not even
  in caves). You walk, mine and build there exactly as at home.
- **One world is active**: `Game::planet`, `terrain`, `streamer` and `gravity` are its, and
  all block code (player physics and collision, interaction, survival, mobs, drops,
  particles) works on it unchanged. Its rotating frame is the frame the game draws in, with
  its centre at `PLANET_CENTER` (both grids are built there); the player's state is relative
  to it.
- **The other world** (`Game::other`) streams no chunks but keeps LOD tiles chosen for the
  camera (converted into its frame), drawn turned into the active frame
  (`DrawItem::rotation`, carried through the far pass): home with its atmosphere in the
  moon's sky, the moon (a bright disc) in home's. Its survival state (models, chests,
  furnaces, drops, animals) waits in `OtherWorld::state` (`Survival::swap_world`).
- **Switching** follows the player's craft (`follow_craft_world`): when the rocket or the
  floating player changes sphere of influence, the worlds swap, the player's state is
  rebased (`Bodies::rebase`) and the view turned (`Camera::rotate_frame`). Dying respawns at
  home.
- Frames: `Bodies::view` / `view_velocity` / `view_rotation` turn any body's positions into
  the active frame. The sky and lighting stay home's: `FrameParams::camera_planet` is the
  camera relative to *home*, the sun direction and star rotation are the active frame's, and
  particles fall towards `gravity_center` (the active world's centre). Moon surfaces are
  lit by the sun (with home's shadow as an eclipse), without sky light.
- **Lunar terrain** (`TerrainGenerator::lunar_column`): dark maria (low basalt plains) and
  pale highlands, craters at four scales (cells of 3.2 km, 1.1 km, 360 m, 110 m: basins down
  to dimples) - bowl, rim and ejecta, hashed per cell of a 3D grid and kept within the
  neighbouring cells so columns always agree. Regolith over moon rock (highlands) or basalt
  (maria), ilmenite (titanium) patches in the maria, lava-tube caves; no water, no trees.
  New blocks: `Regolith`, `MoonRock`, `Basalt`. The LOD tiles show them too.
- Fall damage scales with local gravity (a 7 m moon jump lands like a 1 m one at home).
- Test: `moon_is_grey_dry_and_cratered`.

### Weather (weather/, app/weather.rs, shaders/clouds.frag, weather.glsl)
- **One simulation decides everything.** `weather::sim::Sim` holds a few fields on a cube
  sphere (128 cells per face edge, about 540 m a cell; a cell is exactly a texel of the
  weather maps, Vulkan's cube layout): vapour, cloud, storm, dust, ash. Rain, snow, fog,
  thunder, tornadoes, sandstorms and blizzards aren't separate systems: they are read from
  those fields, so the sky you see and the weather you stand in always agree. It is
  behaviour, not meteorology (the rules are at the top of `sim.rs`):
  - Wind: bands by latitude (trades, westerlies, polar easterlies), drifting noise eddies,
    up to 16 cyclones with spiral rain bands and an eye. Near the ground it is
    `SURFACE_WIND` x the cloud-level wind.
  - Moisture: evaporates off the sea and wet land, saturates sooner in cold air; rising air
    (cyclones, the windward side of mountains, the equator, eddies) condenses it into cloud,
    sinking air clears it. So deserts stay dry and the equator is wet.
  - Thick cloud rains or snows by temperature (climate + season + time of day). Warm moist
    rising air builds storms, which make tall towers and lightning and, over warm land,
    occasionally a tornado. Strong wind over desert lifts dust; volcanoes put out ash.
  - Semi-Lagrangian advection with clamped cubic interpolation (bilinear smears storms).
  - `cargo test --release weather_makes -- --nocapture` prints the planet's mix of weather;
    tune against it. About 40% clear, 2-3% thunderstorm, a few percent each of the rest.
- **Threading:** `Sim` lives on a worker thread. `Weather::update` asks for a step 4 times a
  second (game time, at most 30 s per step) and never waits; finished `Snapshot`s carry the
  two maps, the wind, tornadoes and volcanoes. About 110 steps run before the first
  snapshot (about 2 s), so the game starts with developed weather. Forced weather
  (`weather <kind>`) paints the fields around a point every step.
- **Weather maps** (two RGBA8 cube maps): A = cover, cloud top, precipitation, storm; B =
  dust, snow fraction, fog, ash. They are in home's body frame; `frame.weather_rotation`
  takes a point from the active frame's axes into it, so they work from the moon too.
- **Clouds** (`clouds.frag`, half resolution, `CLOUD_DIVISOR`): a shell 900 m to 5.1 km above
  sea level, marched in 22-56 steps. Weather maps decide where and how tall; two tiling noise
  volumes (`cloud_noise.rs`, tile `SHAPE_TILE` 4.8 km and `DETAIL_TILE` 600 m, their sizes
  must stay multiples) shape it. Light: a short march towards the sun (Beer's law, two-lobe
  phase), plus a spread term for light scattered inside the cloud (it's what makes them
  white). Dark bases and mottling under thick cover.
  - The noise moves by `cloud_offset` = the *local* wind integrated, in 3D and wrapped at
    the tile: right where you are looking from. The large shapes move with the simulation.
  - Output is `rgb` (light added, sun-relative) and `a` (what's left of the background);
    the air in front of each layer is already in `rgb`, so `sky.frag` only does
    `color * a + rgb * sun`. `clouds_resolve.frag` reprojects last frame and clamps it to the
    current neighbourhood. `sky.frag` upsamples with depth-aware weights.
- **The low layer** (`march_haze`, same pass): rain curtains under clouds, fog, dust,
  blown snow, ash, as participating media over a 1.7 km shell. Visible from afar: a
  sandstorm is a wall, a downpour a curtain.
- **Light on the ground** (`cloud_light`, block.frag and particles.frag): direct sun blocked by
  clouds (moving shadows), and what they take comes back as soft sky light. The exposure
  compensates partly: storms stay dark.
- **Northern lights** (`aurora` in clouds.frag): folded curtains in a ring around each pole,
  5.5-15 km up. Additive; only visible when the sky is dark. Off where they can't be seen
  (`cloud_params`).
- **Particles** (`effects/mod.rs`, `weather_particles`): rain with splashes (`particles_sim.comp`
  turns a landing streak into a small splash), snow, blizzard streaks, sand streaks and puffs,
  ash. Rates follow the eased local weather; below the clouds, outdoors only.
  - `flags::ROOFED`: the CPU scans the highest block in a 72 m square around the camera
    (`RoofMap`, a few rows per frame, sent in the particle batch), so weather stops at roofs.
  - Particles over empty sky are composited after the clouds in `sky.frag` (the HDR target's
    alpha is 0 there; blended particles write how much they cover).
- **Lightning:** a bolt is 100-odd short-lived glowing particle bursts along a jagged path;
  each flash also lights the clouds (`flashes`), the ground (`clouds.w`) and sets off a
  thunder sound that arrives at the speed of sound.
- **Tornadoes** (`sim::Tornado`, `app/weather.rs`): a real volume in `clouds.frag`
  (`funnel_sample`: a bending rope flaring into the cloud base, ropes of cloud winding up it,
  a boiling skirt of dust at the foot; shaded by marching towards the light), a vortex
  in `particles_sim.comp` (`wind_at`: whirl, inflow, updraft) that carries dust and bits
  (`drag` > 0 follows it). Near the core its lift beats even the walking player's extra
  gravity: it throws the player and animals hundreds of metres.
  - **Flying blocks** (`effects/debris.rs`): whole blocks as CPU objects. `rip_blocks` tears them out
    of loaded chunks (`can_be_ripped`: trees whole, ground one or two layers deep);
    beyond the loaded area `show_blocks` throws some just for show. They whirl in
    `vortex_wind`, are let go at a height or after a time and flung away, and shatter where
    they land, hurting the player. One cached cube mesh per kind, drawn turned with
    `DrawItem::rotation`.
- **Volcanoes** (terrain): `TerrainGenerator::find_volcanoes`, up to 14, deterministic from
  the seed. `volcano_at` adds a concave gullied cone and a crater with a flat lava lake;
  biomes `Volcanic` (ash, basalt) and `Caldera` (lava). LOD tiles show them, and their
  lava (`Tex::Lava`) carries full block light so it glows at any distance, and shines and
  churns in `block.frag` (`LAVA_LAYER`). Standing on `Block::Lava` burns. Each erupts every
  15-50 game minutes for 3-7 minutes (the first within about 20 minutes of starting):
  - The eruption cloud is a volume in `clouds.frag` (`plume_sample`: rises at 130 m/s, leans
    downwind, spreads out on top), visible 60 km away. Ash also goes into the simulation
    (drifting downwind, ash fall). Up close a lava fountain and lava bombs (`effects/debris.rs`,
    molten blocks on high arcs that splash where they land).
- **Lava flow** (`app/lava.rs`): each eruption starts 3-5 rivers at the rim. A river's path
  is found on the terrain as its front advances (steepest descent, it fills small dips).
  Within the loaded area the front pours real `Block::Lava` onto the ground (burning trees,
  turning to basalt in water), catching up when you come back; beyond it, glowing particles
  run down the same path. After the eruption rivers cool to basalt from the far end, and
  stay. Anything else that opens up next to lava (`WorldStreamer::changes`) makes it run:
  down first, then sideways for a few blocks, and it hardens after a minute or so.
- **Sound** (`audio.rs`): `Sound::Thunder`, and looping `Ambience::{Rain, Wind, Roar}` whose
  volume is set every frame and eases in the mixer. Muffled under a roof and in caves. Their
  levels (`ambience_level`) are RMS, not peak: a full storm stays under a footstep (test
  `weather_stays_under_the_game`).

### Worlds, saving and menus (app/save.rs, ui/menu.rs, app/session.rs)
- **One `Game`, many worlds.** The window, renderer and sound last for the whole run;
  `install_world` replaces every part of a world in place from a `WorldParts` (`build_world`),
  and `Renderer::forget_world` frees all meshes and particles. So: **anything new that
  belongs to a world has to be reset in `install_world`** and, if it should last, saved.
- **The main menu is a world** (`backdrop`): the default seed, seen from orbit, kept over the
  morning side relative to the sun (`update_backdrop`).
- **On disk** (`%APPDATA%\AstroBlock`, see the top of `app/save.rs`): per world `world.json` (for
  the list), `state.json` (`SavedGame`: player, camera, time, inventory, chests, furnaces,
  placed models, drops, animals on both worlds, rockets, maneuver nodes, molten lava) and
  `home.edits` / `moon.edits` (`WorldEdits::to_bytes`). Terrain isn't stored. To add saved
  data: a field on `SavedGame` or its parts; `#[serde(default)]` it so older saves still load.
  - Saving: autosave every 3 minutes (written on another thread), on leaving, on closing
    the window. Files appear whole (written to `.tmp`, then renamed). `read_world` checks
    all of a save before the current world is touched.
  - Light sources aren't stored: they are lit again once the chunks around them have loaded
    (`apply_pending_lights`).
  - Not saved: the weather (it starts fresh from the seed), rivers of lava in mid-flow (what
    was molten cools over the next few minutes), things in the air.
- **Menus** freeze the world (`paused`): `frame` runs with a world `dt` of 0. Page changes,
  world loads and so on go through `Action`s handled between frames. The loading screen
  stays up until chunks, far terrain and weather are ready (`advance_opening`).
- **Settings** (`Settings`, applied live by `apply_settings`): borderless fullscreen, vsync
  (FIFO, or IMMEDIATE/MAILBOX), frame cap (`limit_frame_rate`, sleep then spin), master
  volume, mouse sensitivity and smoothing (`input::MouseSmoother`, all mouse look, the right-drag cameras too). Captures ignore the saved settings and never go fullscreen.

### Multiplayer (net/, app/multiplayer.rs)
- **The table at the top of `app/multiplayer.rs` says who decides what.** The host owns the
  world and its saves. No machine has the whole planet loaded, so each player simulates what
  is around them: animals near them, tornadoes tearing up ground they have loaded. Lava rivers
  aren't sent: every player works out the same ones from the eruption (`start_flow` seeds
  from the volcano and when it started), each pouring blocks where it has chunks.
- **Adding something shared:** a `Msg` (or `WorldEvent`) in `protocol.rs` (bump `PROTOCOL`),
  and its effect in `host_message` / `client_message`. Check anything from the network
  (`valid_cell`, `finite`): a peer must not be able to crash the game.
- **Joining:** the host sends a compressed `WorldBlob` (its `SavedGame` JSON, edits, weather
  maps, the player's `PlayerRecord`) in pieces. Everything reliable after that queues behind
  it for that player, and the client holds it (`early`) until its world is built. Then it
  says `Ready` and gets the live, unreliable traffic.
- **Things have ids** (`Survival::new_id`: player number << 24 | counter), not indices.
- **Survival only reports.** Offline-capable code stays as it was, and `Survival::net` plus
  the `outbox` carry what others must hear; `apply_event` does the same to this copy without
  reporting it. Sounds made here go out from `play_sounds`.
- **Weather:** one simulation per group of players at the same time; `Multiplayer::sky` says
  whose you are watching (your own id: it runs here). Whoever runs it sends one cube face every
  1.5 s (deflated difference from the last one, so a watcher gets every face whole first:
  `WeatherKey`) with a coarse wind and the cyclones; tornadoes and volcanoes go 3 times a
  second. The first time a client needs its own, `Weather::go_local` starts a `Sim` from the
  maps it can see (`Sim::import`; vapour isn't on the maps, so clouds shift a little at first).
- **Time (`app/timeline.rs`):** everyone has their own clock and warp, sent with the pose. More
  than `SAME_TIME` (60 s) from whoever's weather you watch and you run your own. Sync
  (`time_sync`) jumps there and takes their weather; follow also copies their warp. Sharing a
  sky, clocks stay together by running up to 20% fast or slow (`time_scale`) and snap only
  if far off. A jump (`clock_jump`) carries you over the same ground, or along your conic in
  flight (refused if the path comes down first; in flight only forward). `VesselState` says
  when it was true, and `vessel_at` shows that rocket at the viewer's time. New messages about
  anything tied to the clock must carry the sender's time.
- **Teams** (`Multiplayer::team`): pressing a warp key sends `TeamWarp`, and the others on the team
  then follow that player (`Follow::team`) until someone else changes it. Whoever sets the pace is
  held at physics warp while a team mate is flying through air (`keep_team`).
- **The menu doesn't pause a game with others** (`paused`).
- **Steam:** `Client::init_app(480)` at start; nothing else is needed on disk, except that
  `steam_api64.dll` must be next to the executable, even offline (`build.rs` copies it into
  the target folder). A host advertises a connect string (`+astro_host <id>`) by rich
  presence, which is also what invites carry. Only friends can join over Steam.
  Accepting an invite while the game is closed starts the real Spacewar: it needs Astro Block
  to be open already.
- **Direct:** host binds port 47777 (all addresses). Over the internet the host must forward
  that port (TCP and UDP). No encryption and no password: it is for friends.
- Limits: 8 players. Peers are trusted (no anti-cheat). A joining client's world download
  holds up nothing, but building it takes the usual loading time.

### Camera
- The camera sits at the player's eyes (`EYE_HEIGHT` above the feet).
- "Up" is radial. `Camera::set_up` parallel-transports the frame for small changes. When up
  flips (more than 60 degrees in one step, e.g. flying through the planet's centre), it keeps
  the world-space view direction and re-derives yaw/pitch instead. Guard against normalizing a
  zero vector at the exact centre (`try_normalize`).

### First-person view (player/viewmodel.rs)
- In survival and creative, the player's own arm shows at the lower right: deliberately plain and
  blocky (owner's wish), one skin block for hand and forearm and a dark slate shirt sleeve. The
  owner wants a regular hand for now; a space suit can replace it later, when one is worn.
  - It holds the selected item. Sprites (tools, food, ingots...) are extruded one pixel
    thick: a cut-out front and back, plus a rim face per exposed pixel edge. Blocks are a
    small cube. Model blocks (table, furnace, chest, torch) are their box models.
  - It is not drawn in spectator mode.
- The pose is rebuilt every frame in camera space and turned into camera-relative world
  space, so the shader's lighting is right: sun, sky, torches and the camera cell's light.
  The mesh uses `models::VIEW_UNIT` (1/16384 m) so it doesn't jitter. Its parts:
  - arm units = `ARM_UNIT` m, posed at `REST`; the forearm comes up from the bottom of the
    screen (`ARM_YAW`, `ARM_RAISE`) and is never rolled, so it sits level
  - gestures rotate about the far (shoulder) end, plus a wrist turn for the item
  - `grip` places the item in the hand; its orientation is chosen in camera space (what
    you see) and converted to arm space, so moving the arm's rest pose doesn't skew it
- **Animations** (all real time):
  - Swing: a wind-up, a strike that is mostly wrist so the hand stays in view, then a
    recovery. It runs on `Survival::swings` and repeats while mining, with or
    without an item (no separate punch).
  - Use: a push. It runs on `Survival::uses` (placing, opening, putting on).
  - Switching items: the old item lowers out of view, then the new one rises.
  - Eating: the food comes up to the visor and bobs.
  - Also a walk bob, sway behind turning, a dip on landing and breathing.
  - Gestures survive item switches.
  - Smoothness rules: keyframes are one spline (`Pose::keyframes`), not stops at each key.
    A gesture cut short by a new one leaves its pose in `carry`, which fades out. Sway works
    from how far the view moved, not its speed (a mouse's uneven steps would show), through
    a critically damped `Spring`; springs are solved exactly, so any frame rate matches.
    Landings only count after `MIN_AIR_TIME`, so stepping up a block doesn't dip.

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
- The planet has 44,960 layers and 10 shells (surface radius 43.9 km).

## Known limitations / likely next steps
- Particles: no GPU-side spawning yet (rain splashes, sparks that make sparks), no
  collision off screen, and blended particles take the aerial perspective of what is behind
  them (they write no depth). A wind field (rather than one wind vector) and soft
  particles would come with the weather system.
- LOD tiles are a heightfield (one terrain sample per cell), so overhangs and caves don't
  show at distance, and there can be a small visible step where real chunks take over. LOD jobs
  share the worker threads with chunk generation.
- Block light is real (torches), but sky light is still estimated from depth below the column
  surface. There are no terrain shadows (only the planet's own shadow) and no light shafts.
  Shadow maps or a sky-light flood fill would be the next lighting steps.
- The atmosphere is ray-marched per pixel at full resolution. Hillaire's sky-view and
  aerial-perspective LUTs would make it about 3x cheaper if needed. Exposure is a CPU estimate,
  not a luminance histogram.
- Survival is missing these Minecraft parts:
  - hostile mobs (so armor never gets used), shearing, breeding, saplings
  - lava
  - a player model in the inventory screen
  - armor drawn on the player
- Water doesn't flow. Edited cells don't update the fake sky light, and LOD tiles don't show
  edits. Worlds all have the same size; the seed is the only thing that differs.
- Vehicles: no docking or staging, no parachutes, no water (they sink through it). Blocks with
  their own contents (chests, furnaces, torches, crafting tables) can't go on one. Walking aboard
  has no ladders: build steps of blocks. Deck gravity always points at the deck, even upside down.
- The built-in test rocket is still there, with its simpler model.
  - It collides as an upright box at its engine:
    tipping over is a rule (more than ~50 degrees off vertical), not physics. Water isn't
    solid for it.
  - Only the boarded rocket gets full physics; empty ones coast on rails (physics only in
    the air). Floating outside you can't move (no jetpack).
  - Only one block world is loaded at a time: the other keeps far terrain only and its
    furnaces pause.
  - Patched conics only (as KSP): no n-body effects. Map paths around the moon are drawn
    around where the moon is at the encounter.
  - No parachutes or landing legs; the heat model is gameplay-tuned.
- Weather: only home has it (`Weather` is built for one planet's `TerrainGenerator`; another
  planet with air would get its own). No snow or puddles build up on the ground. Wind only
  moves particles and tornado victims (not the player or rockets). Lava is a full glowing
  block you can stand on (it burns): it moves a block at a time, with no partial levels, and
  far LOD terrain doesn't show rivers as blocks (they are particles there).
- Multiplayer: no chat. Lava that only a client saw poured isn't in the host's save, and lava
  rivers only form from the weather the host is watching (so no blocks differ). Blocks and
  explosions are shared at once whatever time you are at: a rocket that crashed in someone's
  future is gone for everyone. Animals
  aren't saved by clients (only the host's own). Each player's lightning bolts are their own.
  The Steam path has never been run between two accounts.

- The desired-set recompute runs on the main thread; it could move to a worker.
- Vulkan validation layers are only active if the LunarG SDK is installed. Install it before
  debugging rendering issues.
- Possible future work: real textures (PNG override of the generated ones), multi-draw indirect,
  occlusion culling for cave chunks.
