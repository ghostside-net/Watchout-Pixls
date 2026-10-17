//! Dynamic Chunk Streaming & Multithreaded Generation via `bevy::tasks` (Phase 2).
//!
//! Manages active chunks around the player's current position:
//! - Spawns parallel background tasks on Bevy's [`AsyncComputeTaskPool`] to generate or restore
//!   chunks entering `load_radius`.
//! - Polls in-flight tasks without blocking via `futures_lite::future::poll_once`.
//! - Unloads distant chunks exceeding `unload_radius`, automatically persisting any modified
//!   chunk into [`WorldPersistence`] while discarding untouched chunks (which can be deterministically
//!   reproduced from [`WorldSeed`]).

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task, TaskPool};
use serde::{Deserialize, Serialize};

use crate::palette_sprite::WorldPixelPos;
use crate::state::{GameLoopMetrics, HeroController};
use crate::window_render::PixelPerfectCamera2d;
use crate::world::coords::{ChunkCoord, TileCoord, WorldGridConfig};
use crate::world::generation::{
    ChunkData, ResourceKind, TerrainTileKind, TileCell, WallKind, WorldGenerator,
};
use crate::world::noise::WorldSeed;
use crate::world::persistence::{ChunkSource, WorldPersistence};

/// Configuration resource controlling dynamic chunk streaming around the player.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkStreamingConfig {
    /// Chebyshev radius (in chunks) around the player within which chunks are actively loaded.
    /// `load_radius = 1` loads a `3x3 = 9` chunk grid; `load_radius = 2` loads a `5x5 = 25` chunk grid.
    pub load_radius: i32,
    /// Chebyshev radius (in chunks) beyond which active chunks are unloaded.
    /// Kept >= `load_radius + 1` to provide hysteresis against border thrashing.
    pub unload_radius: i32,
    /// Maximum number of new chunk generation tasks to dispatch per update tick.
    pub max_tasks_per_frame: usize,
    /// Whether to complete parallel `AsyncComputeTaskPool` tasks within the same frame after
    /// dispatching the batch. Set to `false` to leave unfinished tasks in `in_flight_tasks`
    /// across ticks for multi-frame async inspection.
    pub complete_within_frame: bool,
}

impl Default for ChunkStreamingConfig {
    fn default() -> Self {
        Self {
            load_radius: 1,
            unload_radius: 2,
            max_tasks_per_frame: 16,
            complete_within_frame: true,
        }
    }
}

/// ECS Component attached to an entity representing a currently active world chunk.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkEntity {
    pub coord: ChunkCoord,
    pub source: ChunkSource,
}

/// Diagnostic counters tracking chunk streaming, multithreaded task execution, and persistence.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStreamingMetrics {
    pub active_chunk_count: usize,
    pub in_flight_task_count: usize,
    pub total_async_tasks_spawned: u64,
    pub total_chunks_generated_from_seed: u64,
    pub total_chunks_restored_from_save: u64,
    pub total_chunks_unloaded: u64,
    pub total_modified_chunks_saved: u64,
}

/// Output produced by a background chunk loading/generation task on [`AsyncComputeTaskPool`].
#[derive(Debug, Clone)]
pub struct ChunkTaskOutput {
    pub chunk: ChunkData,
    pub source: ChunkSource,
}

/// Central Bevy Resource holding all currently active chunks, in-flight [`AsyncComputeTaskPool`]
/// tasks, and ECS chunk entity handles.
#[derive(Resource, Default)]
pub struct WorldMap {
    active_chunks: BTreeMap<ChunkCoord, ChunkData>,
    chunk_entities: BTreeMap<ChunkCoord, Entity>,
    in_flight_tasks: BTreeMap<ChunkCoord, Task<ChunkTaskOutput>>,
    pub last_center_chunk: Option<ChunkCoord>,
}

impl WorldMap {
    /// Creates an empty [`WorldMap`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of currently loaded active chunks in memory.
    #[must_use]
    pub fn active_chunk_count(&self) -> usize {
        self.active_chunks.len()
    }

    /// Number of background generation tasks currently in flight on [`AsyncComputeTaskPool`].
    #[must_use]
    pub fn in_flight_task_count(&self) -> usize {
        self.in_flight_tasks.len()
    }

    /// Returns `true` if `coord` is currently loaded in `active_chunks`.
    #[must_use]
    pub fn is_chunk_loaded(&self, coord: ChunkCoord) -> bool {
        self.active_chunks.contains_key(&coord)
    }

    /// Returns `true` if a background task for `coord` is currently running.
    #[must_use]
    pub fn is_chunk_in_flight(&self, coord: ChunkCoord) -> bool {
        self.in_flight_tasks.contains_key(&coord)
    }

    /// Immutable reference to the map of all active chunks.
    #[must_use]
    pub const fn active_chunks(&self) -> &BTreeMap<ChunkCoord, ChunkData> {
        &self.active_chunks
    }

    /// Retrieves an immutable reference to a loaded [`ChunkData`] at `coord`.
    #[must_use]
    pub fn get_chunk(&self, coord: ChunkCoord) -> Option<&ChunkData> {
        self.active_chunks.get(&coord)
    }

    /// Retrieves a mutable reference to a loaded [`ChunkData`] at `coord`.
    pub fn get_chunk_mut(&mut self, coord: ChunkCoord) -> Option<&mut ChunkData> {
        self.active_chunks.get_mut(&coord)
    }

    /// Looks up the [`TileCell`] at global [`TileCoord`] if its containing chunk is currently loaded.
    #[must_use]
    pub fn get_tile(&self, tile: TileCoord, grid: &WorldGridConfig) -> Option<&TileCell> {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile);
        self.active_chunks.get(&chunk_coord)?.get_tile(local)
    }

    /// Replaces the [`TileCell`] at global [`TileCoord`] if the chunk is loaded, marking the chunk as modified.
    pub fn set_tile(&mut self, tile: TileCoord, grid: &WorldGridConfig, cell: TileCell) -> bool {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile);
        let Some(chunk) = self.active_chunks.get_mut(&chunk_coord) else {
            return false;
        };
        chunk.set_tile(local, cell)
    }

    /// Places or clears a [`WallKind`] at global [`TileCoord`] if the chunk is loaded, marking it as modified.
    pub fn set_wall(
        &mut self,
        tile: TileCoord,
        grid: &WorldGridConfig,
        wall: Option<WallKind>,
    ) -> bool {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile);
        let Some(chunk) = self.active_chunks.get_mut(&chunk_coord) else {
            return false;
        };
        chunk.set_wall(local, wall)
    }

    /// Updates the [`TerrainTileKind`] at global [`TileCoord`] if the chunk is loaded, marking it as modified.
    pub fn set_terrain(
        &mut self,
        tile: TileCoord,
        grid: &WorldGridConfig,
        terrain: TerrainTileKind,
    ) -> bool {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile);
        let Some(chunk) = self.active_chunks.get_mut(&chunk_coord) else {
            return false;
        };
        chunk.set_terrain(local, terrain)
    }

    /// Harvests up to `amount` units from a resource node at global [`TileCoord`] if the chunk is loaded.
    /// Marks the chunk as modified when a resource is harvested.
    pub fn harvest_resource(
        &mut self,
        tile: TileCoord,
        grid: &WorldGridConfig,
        amount: u16,
    ) -> Option<(ResourceKind, u16)> {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile);
        let chunk = self.active_chunks.get_mut(&chunk_coord)?;
        chunk.harvest_resource(local, amount)
    }

    /// Persists all currently active modified chunks into `persistence` without unloading them.
    pub fn save_active_modified_chunks(&self, persistence: &mut WorldPersistence) -> usize {
        let mut count = 0;
        for chunk in self.active_chunks.values() {
            if persistence.store_chunk_if_modified(chunk) {
                count += 1;
            }
        }
        count
    }

    /// Unloads all active chunks (saving any modified chunks into `persistence`) and clears in-flight tasks.
    pub fn unload_all_chunks(
        &mut self,
        commands: &mut Commands,
        persistence: &mut WorldPersistence,
        metrics: &mut WorldStreamingMetrics,
    ) {
        let coords: Vec<ChunkCoord> = self.active_chunks.keys().copied().collect();
        for coord in coords {
            if let Some(chunk) = self.active_chunks.remove(&coord) {
                if persistence.store_chunk_if_modified(&chunk) {
                    metrics.total_modified_chunks_saved += 1;
                }
                metrics.total_chunks_unloaded += 1;
            }
            if let Some(entity) = self.chunk_entities.remove(&coord) {
                commands.entity(entity).despawn();
            }
        }
        self.in_flight_tasks.clear();
        self.last_center_chunk = None;
        metrics.active_chunk_count = 0;
        metrics.in_flight_task_count = 0;
    }
}

/// Spawns a background task on Bevy's [`AsyncComputeTaskPool`] to either restore a saved modified
/// chunk or deterministically generate a fresh chunk from `seed`.
fn spawn_chunk_task(
    coord: ChunkCoord,
    seed: WorldSeed,
    grid_config: WorldGridConfig,
    saved_modified: Option<ChunkData>,
) -> Task<ChunkTaskOutput> {
    let pool = AsyncComputeTaskPool::get_or_init(TaskPool::default);
    pool.spawn(async move {
        if let Some(chunk) = saved_modified {
            ChunkTaskOutput {
                chunk,
                source: ChunkSource::RestoredModified,
            }
        } else {
            let generator = WorldGenerator::new(seed);
            let chunk = generator.generate_chunk(coord, &grid_config);
            ChunkTaskOutput {
                chunk,
                source: ChunkSource::GeneratedFromSeed,
            }
        }
    })
}

/// System that determines the player's current chunk, unloads distant chunks outside
/// `unload_radius` (persisting any modified state), and dispatches parallel generation tasks
/// on [`AsyncComputeTaskPool`] for missing chunks within `load_radius`.
#[allow(clippy::too_many_arguments)]
pub fn queue_chunk_streaming_system(
    mut commands: Commands,
    seed: Res<WorldSeed>,
    grid_config: Res<WorldGridConfig>,
    streaming_config: Res<ChunkStreamingConfig>,
    mut world_map: ResMut<WorldMap>,
    mut persistence: ResMut<WorldPersistence>,
    mut metrics: ResMut<WorldStreamingMetrics>,
    scene_manager: Option<Res<crate::scene::SceneManager>>,
    hero_query: Query<&WorldPixelPos, With<HeroController>>,
    camera_query: Query<&PixelPerfectCamera2d>,
) {
    // Pause Overworld chunk streaming/unloading while the player is inside an instanced scene
    if scene_manager
        .as_deref()
        .is_some_and(crate::scene::SceneManager::is_instance)
    {
        return;
    }

    // Determine center world position from the Hero entity (or camera center fallback)
    let center_px = if let Some(hero_pos) = hero_query.iter().next() {
        hero_pos.to_vec2()
    } else if let Some(cam) = camera_query.iter().next() {
        cam.position + cam.viewport_size.as_vec2() * 0.5
    } else {
        Vec2::ZERO
    };

    let center_chunk = grid_config.world_pixel_to_chunk(center_px);
    world_map.last_center_chunk = Some(center_chunk);

    let unload_radius = streaming_config
        .unload_radius
        .max(streaming_config.load_radius) as u32;

    // 1. Unload distant active chunks outside unload_radius
    let distant_loaded: Vec<ChunkCoord> = world_map
        .active_chunks
        .keys()
        .copied()
        .filter(|c| c.chebyshev_distance(center_chunk) > unload_radius)
        .collect();

    for coord in distant_loaded {
        if let Some(chunk) = world_map.active_chunks.remove(&coord) {
            if persistence.store_chunk_if_modified(&chunk) {
                metrics.total_modified_chunks_saved += 1;
            }
            metrics.total_chunks_unloaded += 1;
        }
        if let Some(entity) = world_map.chunk_entities.remove(&coord) {
            commands.entity(entity).despawn();
        }
    }

    // Cancel any in-flight tasks that are now outside unload_radius
    world_map
        .in_flight_tasks
        .retain(|c, _| c.chebyshev_distance(center_chunk) <= unload_radius);

    // 2. Queue missing chunks inside load_radius (sorted closest-first)
    let desired_chunks = center_chunk.chunks_in_radius(streaming_config.load_radius);
    let mut spawned_this_tick = 0usize;

    for coord in desired_chunks {
        if spawned_this_tick >= streaming_config.max_tasks_per_frame {
            break;
        }
        if world_map.is_chunk_loaded(coord) || world_map.is_chunk_in_flight(coord) {
            continue;
        }

        let saved_modified = persistence.get_modified_chunk(coord).cloned();
        let task = spawn_chunk_task(coord, *seed, *grid_config, saved_modified);
        world_map.in_flight_tasks.insert(coord, task);
        metrics.total_async_tasks_spawned += 1;
        spawned_this_tick += 1;
    }

    metrics.active_chunk_count = world_map.active_chunks.len();
    metrics.in_flight_task_count = world_map.in_flight_tasks.len();
}

/// System that harvests completed chunk generation tasks from [`AsyncComputeTaskPool`],
/// inserts their [`ChunkData`] into [`WorldMap`], records exploration in [`WorldPersistence`],
/// and spawns [`ChunkEntity`] markers.
pub fn poll_chunk_streaming_tasks_system(
    mut commands: Commands,
    streaming_config: Res<ChunkStreamingConfig>,
    mut world_map: ResMut<WorldMap>,
    mut persistence: ResMut<WorldPersistence>,
    mut metrics: ResMut<WorldStreamingMetrics>,
    loop_metrics: Option<Res<GameLoopMetrics>>,
) {
    let current_tick = loop_metrics.map_or(0, |m| m.in_game_ticks);

    // First pass: non-blocking poll_once on all in-flight tasks
    let mut finished_coords = Vec::new();
    let mut outputs = Vec::new();

    for (&coord, task) in &mut world_map.in_flight_tasks {
        if let Some(output) = block_on(future::poll_once(task)) {
            finished_coords.push(coord);
            outputs.push(output);
        }
    }

    for coord in finished_coords {
        world_map.in_flight_tasks.remove(&coord);
    }

    // Optional deterministic frame barrier: await any remaining parallel tasks from this batch
    if streaming_config.complete_within_frame && !world_map.in_flight_tasks.is_empty() {
        let remaining_tasks = std::mem::take(&mut world_map.in_flight_tasks);
        for (_coord, task) in remaining_tasks {
            outputs.push(block_on(task));
        }
    }

    // Register completed chunks into WorldMap & WorldPersistence
    for output in outputs {
        let coord = output.chunk.coord;
        match output.source {
            ChunkSource::GeneratedFromSeed => {
                metrics.total_chunks_generated_from_seed += 1;
            }
            ChunkSource::RestoredModified => {
                metrics.total_chunks_restored_from_save += 1;
            }
        }

        persistence.mark_chunk_explored(&output.chunk, current_tick);
        world_map.active_chunks.insert(coord, output.chunk);

        let entity = commands
            .spawn(ChunkEntity {
                coord,
                source: output.source,
            })
            .id();
        world_map.chunk_entities.insert(coord, entity);
    }

    metrics.active_chunk_count = world_map.active_chunks.len();
    metrics.in_flight_task_count = world_map.in_flight_tasks.len();
}

/// Cleanup system executed when exiting `GameState::InGame` to persist any modified active
/// chunks before resetting active chunk entities.
pub fn persist_and_clear_active_chunks_on_exit_system(
    mut commands: Commands,
    mut world_map: ResMut<WorldMap>,
    mut persistence: ResMut<WorldPersistence>,
    mut metrics: ResMut<WorldStreamingMetrics>,
) {
    world_map.unload_all_chunks(&mut commands, &mut persistence, &mut metrics);
}
