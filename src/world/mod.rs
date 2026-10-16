//! Core World System (Phase 2): Coordinates, Procedural Noise Generation,
//! Dynamic Chunk Streaming (`bevy::tasks`), and Binary/Compressed Persistence.
//!
//! # Submodules
//! - [`coords`]: Tile (`16x16` px) and Chunk (`16x16` tiles) coordinate systems and Euclidean grid math.
//! - [`noise`]: Deterministic seed-based 2D Simplex, Perlin, FBm, Ridged Multifractal, and coordinate hashing.
//! - [`generation`]: Procedural generation of biomes, winding roads/paths, walls, and harvestable resources.
//! - [`streaming`]: Multithreaded chunk loading/unloading around the player via `AsyncComputeTaskPool`.
//! - [`persistence`]: Efficient serialization of modified chunks and explored areas (`Binary`, `CompressedRon`, `CompressedJson`, `Ron`, `Json`).

pub mod coords;
pub mod generation;
pub mod noise;
pub mod persistence;
pub mod streaming;

pub use coords::{
    ChunkCoord, GridError, LocalTileCoord, TileCoord, WorldGridConfig, DEFAULT_CHUNK_SIZE_TILES,
    DEFAULT_TILE_SIZE_PX,
};
pub use generation::{
    BiomeKind, ChunkData, ResourceKind, ResourceNode, TerrainTileKind, TileCell, WallKind,
    WorldGenerator,
};
pub use noise::{splitmix64, FastNoise2d, WorldSeed};
pub use persistence::{
    ChunkSource, ExploredChunkRecord, PersistenceError, SaveFormat, TileDelta, WorldPersistence,
    WorldSaveData, BINARY_SAVE_MAGIC, WORLD_SAVE_FORMAT_VERSION,
};
pub use streaming::{
    persist_and_clear_active_chunks_on_exit_system, poll_chunk_streaming_tasks_system,
    queue_chunk_streaming_system, ChunkEntity, ChunkStreamingConfig, ChunkTaskOutput, WorldMap,
    WorldStreamingMetrics,
};

use bevy::prelude::*;

use crate::content::{GameContentRegistries, TileDefinition};
use crate::palette_sprite::{PastelGameBoyPalette, RgbaColor, WorldPixelPos};
use crate::state::{GameState, HeroController};
use crate::window_render::{PixelCanvas, PixelPerfectCamera2d};

/// Event for moving or teleporting the playable Hero in world space.
#[derive(Event, Debug, Clone, Copy, PartialEq)]
pub enum HeroMoveEvent {
    /// Moves the hero by a delta vector `(dx, dy)` in world pixels (respecting tile walkability if `check_collision` is true).
    TranslatePixels { delta: Vec2, check_collision: bool },
    /// Teleports the hero directly to the center of a target [`TileCoord`].
    TeleportToTile(TileCoord),
    /// Teleports the hero directly to world pixel coordinates.
    TeleportToPixel(Vec2),
}

/// System that processes [`HeroMoveEvent`]s and updates the Hero's [`WorldPixelPos`].
pub fn handle_hero_move_events_system(
    mut events: EventReader<HeroMoveEvent>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    mut hero_query: Query<&mut WorldPixelPos, With<HeroController>>,
) {
    let Some(mut hero_pos) = hero_query.iter_mut().next() else {
        events.clear();
        return;
    };

    for event in events.read() {
        match *event {
            HeroMoveEvent::TranslatePixels {
                delta,
                check_collision,
            } => {
                let target_px = hero_pos.to_vec2() + delta;
                if check_collision {
                    let target_tile = grid_config.world_pixel_to_tile(target_px);
                    let walkable = world_map
                        .get_tile(target_tile, &grid_config)
                        .is_none_or(TileCell::is_walkable);
                    if walkable {
                        *hero_pos = WorldPixelPos::new(target_px.x, target_px.y);
                    }
                } else {
                    *hero_pos = WorldPixelPos::new(target_px.x, target_px.y);
                }
            }
            HeroMoveEvent::TeleportToTile(tile) => {
                *hero_pos = grid_config.tile_center_world_pixel(tile);
            }
            HeroMoveEvent::TeleportToPixel(px) => {
                *hero_pos = WorldPixelPos::new(px.x, px.y);
            }
        }
    }
}

/// System that keeps [`PixelPerfectCamera2d`] centered on the Hero during `GameState::InGame`.
/// At the initial Hero spawn `(152.0, 112.0)`, camera position is `(0.0, 0.0)`.
pub fn sync_camera_to_hero_system(
    hero_query: Query<&WorldPixelPos, With<HeroController>>,
    mut camera_query: Query<&mut PixelPerfectCamera2d>,
) {
    let Some(hero_pos) = hero_query.iter().next() else {
        return;
    };
    let Some(mut camera) = camera_query.iter_mut().next() else {
        return;
    };
    camera.position = Vec2::new(hero_pos.x - 152.0, hero_pos.y - 112.0);
}

/// Registers additional Phase 2 terrain [`TileDefinition`]s into [`GameContentRegistries`].
pub fn register_phase2_world_tiles(registries: &mut GameContentRegistries) {
    let extra_tiles = [
        TileDefinition {
            id: "tile.pastel_path".to_string(),
            display_name: "Winding Pastel Path".to_string(),
            walkable: true,
            transparent: true,
            movement_cost: 0.8,
            sprite_id: "tile_pastel_path".to_string(),
            custom_indices_8x8: None,
        },
        TileDefinition {
            id: "tile.buttercup_sand".to_string(),
            display_name: "Buttercup Sand Bank".to_string(),
            walkable: true,
            transparent: true,
            movement_cost: 1.2,
            sprite_id: "tile_buttercup_sand".to_string(),
            custom_indices_8x8: None,
        },
        TileDefinition {
            id: "tile.lavender_floor".to_string(),
            display_name: "Lavender Woodland Moss".to_string(),
            walkable: true,
            transparent: true,
            movement_cost: 1.0,
            sprite_id: "tile_lavender_floor".to_string(),
            custom_indices_8x8: None,
        },
        TileDefinition {
            id: "tile.deep_water".to_string(),
            display_name: "Deep Breeze Lake".to_string(),
            walkable: false,
            transparent: true,
            movement_cost: 99.0,
            sprite_id: "tile_deep_water".to_string(),
            custom_indices_8x8: None,
        },
    ];

    for def in extra_tiles {
        let _ = registries.tiles.register_or_override(def);
    }
}

/// Renders all visible chunk tiles inside the camera's viewport onto the native [`PixelCanvas`].
/// Uses strict frustum/viewport culling so only tiles overlapping the screen are drawn.
pub fn render_visible_chunks_to_canvas(
    canvas: &mut PixelCanvas,
    palette: &PastelGameBoyPalette,
    camera: &PixelPerfectCamera2d,
    world_map: &WorldMap,
    grid_config: &WorldGridConfig,
) {
    if world_map.active_chunk_count() == 0 {
        return;
    }

    let origin = camera.snapped_origin();
    let view_w = canvas.native_width() as i32;
    let view_h = canvas.native_height() as i32;

    let min_tile = grid_config.world_pixel_to_tile(Vec2::new(origin.x as f32, origin.y as f32));
    let max_tile = grid_config.world_pixel_to_tile(Vec2::new(
        (origin.x + view_w) as f32,
        (origin.y + view_h) as f32,
    ));

    let tile_px = grid_config.tile_size_px;

    for ty in min_tile.y..=max_tile.y {
        for tx in min_tile.x..=max_tile.x {
            let tile_coord = TileCoord::new(tx, ty);
            let Some(cell) = world_map.get_tile(tile_coord, grid_config) else {
                continue;
            };
            let world_px = grid_config.tile_to_world_pixel(tile_coord);
            let screen_px = camera.world_to_screen_pixel(world_px.to_vec2());
            blit_tile_cell_16x16(
                canvas,
                palette,
                screen_px.x,
                screen_px.y,
                tile_px,
                *cell,
                tile_coord,
            );
        }
    }
}

/// Draws a single `tile_px x tile_px` (default `16x16`) procedural [`TileCell`] using
/// deterministic Pastel GameBoy palette indices (`0..8`).
fn blit_tile_cell_16x16(
    canvas: &mut PixelCanvas,
    palette: &PastelGameBoyPalette,
    screen_x: i32,
    screen_y: i32,
    tile_px: u32,
    cell: TileCell,
    coord: TileCoord,
) {
    let lookup = |idx: u8| -> RgbaColor {
        palette
            .lookup(idx % (palette.len() as u8))
            .unwrap_or(RgbaColor::BLACK)
    };

    // Base ground palette index + secondary detail index
    let (base_idx, accent_idx): (u8, u8) = match cell.terrain {
        TerrainTileKind::MeadowGrass => (1, 2), // Mint Pastel + Sage Leaf
        TerrainTileKind::DirtPath => (0, 6),    // Cream Mist + Buttercup Sun
        TerrainTileKind::ShallowWater => (7, 0), // Sky Breeze + Cream ripple
        TerrainTileKind::DeepWater => (7, 2),   // Sky Breeze + Sage depth
        TerrainTileKind::SandBank => (6, 0),    // Buttercup Sun + Cream Mist
        TerrainTileKind::LavenderMoss => (5, 2), // Lavender Haze + Sage Leaf
        TerrainTileKind::CobbleFloor => (5, 3), // Lavender Stone + Deep Slate
        TerrainTileKind::SunlitGrass => (1, 6), // Mint Pastel + Buttercup Sun
    };

    let base_color = lookup(base_idx);
    let accent_color = lookup(accent_idx);
    let ink_color = lookup(3); // Deep Slate Berry contour

    let pattern_seed = ((coord.x.wrapping_mul(37) ^ coord.y.wrapping_mul(101)) & 7) as u32;

    for py in 0..tile_px {
        for px in 0..tile_px {
            let mut color = base_color;

            // Subtle deterministic pixel-art grass/water/stone texture
            if ((px + pattern_seed).is_multiple_of(5) && (py + pattern_seed).is_multiple_of(5))
                || ((px ^ py) == 3 && (px + py).is_multiple_of(4))
            {
                color = accent_color;
            }

            // Path border stones
            if cell.terrain == TerrainTileKind::DirtPath
                && (px == 0 || py == 0 || px == tile_px - 1 || py == tile_px - 1)
                && (px + py).is_multiple_of(3)
            {
                color = lookup(5);
            }

            // Wall overlay (solid masonry/rock structure with slate outline)
            if let Some(wall) = cell.wall {
                let is_edge = px == 0 || py == 0 || px == tile_px - 1 || py == tile_px - 1;
                let is_mortar = py == tile_px / 2 || (py < tile_px / 2 && px == tile_px / 2);
                color = if is_edge || is_mortar {
                    ink_color
                } else {
                    match wall {
                        WallKind::CobbleWall => lookup(5),
                        WallKind::CliffRock => lookup(2),
                        WallKind::AncientPillar => lookup(6),
                    }
                };
            } else if let Some(res) = cell.resource {
                // Centered 8x8 resource sprite overlay inside the 16x16 tile
                let margin = tile_px / 4;
                if px >= margin && px < tile_px - margin && py >= margin && py < tile_px - margin {
                    let rx = px - margin;
                    let ry = py - margin;
                    let span = tile_px - 2 * margin;
                    let is_res_border = rx == 0 || ry == 0 || rx == span - 1 || ry == span - 1;
                    color = if is_res_border {
                        ink_color
                    } else {
                        match res.kind {
                            ResourceKind::MintHerb => lookup(2),
                            ResourceKind::LavenderTimber => lookup(5),
                            ResourceKind::SlateOre => lookup(3),
                            ResourceKind::CrystalCluster => lookup(7),
                            ResourceKind::BerryBush => lookup(4),
                        }
                    };
                }
            }

            canvas.set_pixel(screen_x + px as i32, screen_y + py as i32, color);
        }
    }
}

/// Bevy Plugin registering all Phase 2 Core World resources, events, and systems.
#[derive(Debug, Clone, Default)]
pub struct WorldPlugin {
    pub seed: WorldSeed,
    pub grid_config: WorldGridConfig,
    pub streaming_config: ChunkStreamingConfig,
}

impl WorldPlugin {
    #[must_use]
    pub fn new(
        seed: WorldSeed,
        grid_config: WorldGridConfig,
        streaming_config: ChunkStreamingConfig,
    ) -> Self {
        Self {
            seed,
            grid_config,
            streaming_config,
        }
    }
}

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        self.grid_config
            .validate()
            .expect("WorldGridConfig must have non-zero tile_size_px and chunk_size_tiles");

        if app.world().contains_resource::<GameContentRegistries>() {
            let mut registries = app.world_mut().resource_mut::<GameContentRegistries>();
            register_phase2_world_tiles(&mut registries);
        }

        app.insert_resource(self.seed)
            .insert_resource(self.grid_config)
            .insert_resource(self.streaming_config)
            .init_resource::<WorldMap>()
            .init_resource::<WorldPersistence>()
            .init_resource::<WorldStreamingMetrics>()
            .add_event::<HeroMoveEvent>()
            .add_systems(
                OnEnter(GameState::InGame),
                (
                    queue_chunk_streaming_system,
                    poll_chunk_streaming_tasks_system,
                )
                    .chain(),
            )
            .add_systems(
                OnExit(GameState::InGame),
                persist_and_clear_active_chunks_on_exit_system,
            )
            .add_systems(
                Update,
                (
                    handle_hero_move_events_system,
                    sync_camera_to_hero_system,
                    queue_chunk_streaming_system,
                    poll_chunk_streaming_tasks_system,
                )
                    .chain()
                    .run_if(in_state(GameState::InGame)),
            );
    }
}
