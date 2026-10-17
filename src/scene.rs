//! Modular Scene & Instance Management, Procedural BSP Dungeons, and Static Prefabs (Phase 3).
//!
//! Provides:
//! - [`SceneManager`] and [`Scene`] (`Scene::Overworld(OverworldState)`, `Scene::Instance(DungeonState)`).
//! - Deterministic [`generate_bsp_dungeon`] using Binary Space Partitioning (BSP) + Flood-Fill connectivity guarantee.
//! - File- and string-based [`PrefabLoader`] supporting RON, JSON, and ASCII layouts with strict validation
//!   and corruption fallback.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::Path;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{DataFormat, GameContentRegistries};
use crate::palette_sprite::{PastelGameBoyPalette, RgbaColor};
use crate::player::{CollisionMap, FacingDirection};
use crate::window_render::{PixelCanvas, PixelPerfectCamera2d};
use crate::world::coords::{ChunkCoord, TilePos, WorldGridConfig, WorldPos, DEFAULT_TILE_SIZE_PX};
use crate::world::generation::{ChunkData, TerrainTileKind, TileCollision, WallKind};
use crate::world::noise::{splitmix64, WorldSeed};

/// Minimum allowed width or height (in tiles) for procedural BSP dungeon generation.
pub const MIN_BSP_DUNGEON_DIMENSION: u32 = 12;

/// Built-in valid tile IDs accepted by the prefab validator even when no custom registry is supplied.
pub const BUILTIN_VALID_TILE_IDS: &[&str] = &[
    "tile.pastel_meadow",
    "tile.pastel_cobble",
    "tile.pastel_path",
    "tile.buttercup_sand",
    "tile.lavender_floor",
    "tile.deep_water",
    "tile.sky_stream",
];

/// Errors that can occur during procedural BSP dungeon generation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DungeonGenError {
    #[error("Dungeon dimensions {width}x{height} are too small (minimum is {MIN_BSP_DUNGEON_DIMENSION}x{MIN_BSP_DUNGEON_DIMENSION})")]
    DimensionsTooSmall { width: u32, height: u32 },
    #[error("Invalid tile size: {0}")]
    InvalidTileSize(u32),
}

/// Errors that can occur when loading or validating a static room/house prefab.
#[derive(Debug, Error)]
pub enum PrefabError {
    #[error("Prefab ID must not be empty")]
    EmptyId,
    #[error("Prefab display name must not be empty for '{id}'")]
    EmptyDisplayName { id: String },
    #[error("Invalid prefab dimensions {width}x{height} for '{id}' (must be at least 3x3)")]
    InvalidDimensions { id: String, width: u32, height: u32 },
    #[error(
        "Prefab '{id}' row count {actual_rows} does not match declared height {expected_height}"
    )]
    RowCountMismatch {
        id: String,
        expected_height: u32,
        actual_rows: usize,
    },
    #[error("Prefab '{id}' row {row} width {actual_width} does not match declared width {expected_width}")]
    ColumnCountMismatch {
        id: String,
        row: usize,
        expected_width: u32,
        actual_width: usize,
    },
    #[error("Unknown ASCII tile symbol '{symbol}' at ({x}, {y}) in prefab '{id}'")]
    UnknownTileSymbol {
        id: String,
        symbol: char,
        x: u32,
        y: u32,
    },
    #[error("Unknown tile ID '{tile_id}' referenced in legend of prefab '{id}'")]
    UnknownTileId { id: String, tile_id: String },
    #[error("Missing valid spawn point in prefab '{id}'")]
    MissingSpawnPoint { id: String },
    #[error("Spawn point ({x}, {y}) is out of bounds or on a blocking tile in prefab '{id}'")]
    InvalidSpawnPoint { id: String, x: i32, y: i32 },
    #[error("Exit point ({x}, {y}) is out of bounds or on a blocking tile in prefab '{id}'")]
    InvalidExitPoint { id: String, x: i32, y: i32 },
    #[error("Unsupported prefab data format: {0}")]
    UnsupportedFormat(String),
    #[error("I/O error reading prefab file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse RON prefab: {0}")]
    RonParse(String),
    #[error("Failed to parse JSON prefab: {0}")]
    JsonParse(String),
}

/// Snapshot of the persistent Overworld state saved when entering an instanced scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverworldState {
    /// Exact world pixel position where the player will respawn upon leaving the instance.
    pub return_position: Vec2,
    /// Player's facing direction upon returning to the Overworld.
    pub return_facing: FacingDirection,
    /// Seed of the Overworld generator.
    pub world_seed: WorldSeed,
    /// Cached active chunks around the entrance at the moment of transition.
    pub cached_chunks: BTreeMap<ChunkCoord, ChunkData>,
}

impl Default for OverworldState {
    fn default() -> Self {
        Self {
            return_position: Vec2::new(152.0, 112.0),
            return_facing: FacingDirection::South,
            world_seed: WorldSeed::default(),
            cached_chunks: BTreeMap::new(),
        }
    }
}

/// Origin type of an instanced scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceSource {
    /// Procedurally generated BSP dungeon from a deterministic 64-bit seed.
    ProceduralBsp { seed: u64 },
    /// Loaded from a static prefab definition or asset file.
    Prefab {
        prefab_id: String,
        source_path: Option<String>,
    },
}

/// A single tile cell inside an instanced map ([`DungeonState`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceTile {
    pub tile_id: String,
    pub terrain: TerrainTileKind,
    pub wall: Option<WallKind>,
    pub collision: TileCollision,
    pub is_spawn: bool,
    pub is_exit: bool,
}

impl InstanceTile {
    /// Creates a solid wall tile (`TileCollision::Solid`).
    #[must_use]
    pub fn solid_wall(wall_kind: WallKind) -> Self {
        Self {
            tile_id: "tile.pastel_cobble".to_string(),
            terrain: TerrainTileKind::CobbleFloor,
            wall: Some(wall_kind),
            collision: TileCollision::Solid,
            is_spawn: false,
            is_exit: false,
        }
    }

    /// Creates a walkable floor tile (`TileCollision::None`).
    #[must_use]
    pub fn walkable_floor(terrain: TerrainTileKind) -> Self {
        Self {
            tile_id: terrain.content_id().to_string(),
            terrain,
            wall: None,
            collision: TileCollision::None,
            is_spawn: false,
            is_exit: false,
        }
    }

    /// Creates a deep water hazard tile (`TileCollision::Water`).
    #[must_use]
    pub fn water() -> Self {
        Self {
            tile_id: "tile.deep_water".to_string(),
            terrain: TerrainTileKind::DeepWater,
            wall: None,
            collision: TileCollision::Water,
            is_spawn: false,
            is_exit: false,
        }
    }
}

/// Rectangular room carved by the BSP dungeon generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BspRoom {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BspRoom {
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Center [`TilePos`] of the room.
    #[must_use]
    pub const fn center(&self) -> TilePos {
        TilePos::new(
            (self.x + self.width / 2) as i32,
            (self.y + self.height / 2) as i32,
        )
    }

    /// Returns `true` if `pos` lies inside the room's floor area.
    #[must_use]
    pub const fn contains(&self, pos: TilePos) -> bool {
        pos.0 >= self.x as i32
            && pos.0 < (self.x + self.width) as i32
            && pos.1 >= self.y as i32
            && pos.1 < (self.y + self.height) as i32
    }
}

/// Interactive or decorative prop placed inside a [`DungeonState`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstancePropKind {
    /// Interactive exit ladder/portal/door leading back to the Overworld.
    ExitPortal { label: String },
    /// Lootable treasure chest.
    TreasureChest {
        chest_id: String,
        loot_item_id: String,
        opened: bool,
    },
    /// Friendly NPC residing inside the room/house.
    Npc {
        npc_id: String,
        display_name: String,
        dialogue: String,
    },
    /// Non-blocking interior decoration (rug, table, lantern).
    Decoration { id: String },
}

/// Positioned prop inside an instanced scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceProp {
    pub tile: TilePos,
    pub kind: InstancePropKind,
}

/// State of an instanced interior scene (procedural BSP dungeon or static prefab room).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DungeonState {
    pub instance_id: String,
    pub display_name: String,
    pub source: InstanceSource,
    pub width: u32,
    pub height: u32,
    pub tile_size_px: u32,
    pub tiles: Vec<InstanceTile>,
    pub spawn_tile: TilePos,
    pub exit_tile: TilePos,
    pub rooms: Vec<BspRoom>,
    pub props: Vec<InstanceProp>,
}

impl DungeonState {
    /// Checks whether `pos` is within `[0..width, 0..height)`.
    #[must_use]
    pub const fn in_bounds(&self, pos: TilePos) -> bool {
        pos.0 >= 0 && pos.1 >= 0 && (pos.0 as u32) < self.width && (pos.1 as u32) < self.height
    }

    /// Returns the [`InstanceTile`] at `pos`, or `None` if out of bounds.
    #[must_use]
    pub fn get_tile(&self, pos: TilePos) -> Option<&InstanceTile> {
        if !self.in_bounds(pos) {
            return None;
        }
        let idx = (pos.1 as u32 * self.width + pos.0 as u32) as usize;
        self.tiles.get(idx)
    }

    /// Mutably accesses the [`InstanceTile`] at `pos`, or `None` if out of bounds.
    pub fn get_tile_mut(&mut self, pos: TilePos) -> Option<&mut InstanceTile> {
        if !self.in_bounds(pos) {
            return None;
        }
        let idx = (pos.1 as u32 * self.width + pos.0 as u32) as usize;
        self.tiles.get_mut(idx)
    }

    /// Evaluates [`TileCollision`] at `pos`, deterministically returning [`TileCollision::Solid`]
    /// for any out-of-bounds coordinate without panicking.
    #[must_use]
    pub fn collision_at(&self, pos: TilePos) -> TileCollision {
        self.get_tile(pos)
            .map_or(TileCollision::Solid, |t| t.collision)
    }

    /// Computes the world pixel spawn coordinate for the Hero inside this instance
    /// (centered within `spawn_tile` so an 8x8 sprite and its hitbox sit cleanly inside the tile).
    #[must_use]
    pub fn spawn_world_pos(&self) -> WorldPos {
        self.tile_to_entity_world_pos(self.spawn_tile)
    }

    /// Computes the world pixel coordinate of the instance's exit trigger tile.
    #[must_use]
    pub fn exit_world_pos(&self) -> WorldPos {
        self.tile_to_entity_world_pos(self.exit_tile)
    }

    /// Converts an instance [`TilePos`] to an entity [`WorldPos`] centered inside the `tile_size_px` tile.
    #[must_use]
    pub fn tile_to_entity_world_pos(&self, tile: TilePos) -> WorldPos {
        let ts = self.tile_size_px as f32;
        let offset = (ts - 8.0).max(0.0) * 0.5;
        WorldPos::new(tile.0 as f32 * ts + offset, tile.1 as f32 * ts + offset)
    }

    /// Performs a Breadth-First Search (Flood-Fill) starting from `start` over all walkable tiles
    /// (`TileCollision::None`) and returns the set of all reachable [`TilePos`] coordinates.
    #[must_use]
    pub fn flood_fill_reachable_from(&self, start: TilePos) -> BTreeSet<TilePos> {
        let mut visited = BTreeSet::new();
        if !self.collision_at(start).is_walkable() {
            return visited;
        }

        let mut queue = VecDeque::new();
        visited.insert(start);
        queue.push_back(start);

        while let Some(curr) = queue.pop_front() {
            for next in curr.cardinal_neighbors() {
                if !visited.contains(&next) && self.collision_at(next).is_walkable() {
                    visited.insert(next);
                    queue.push_back(next);
                }
            }
        }

        visited
    }

    /// Verifies via Flood-Fill that there is a continuous walkable path from `spawn_tile`
    /// to `exit_tile` AND to every room center in `rooms`.
    #[must_use]
    pub fn is_connected_spawn_to_exit(&self) -> bool {
        let reachable = self.flood_fill_reachable_from(self.spawn_tile);
        if !reachable.contains(&self.exit_tile) {
            return false;
        }
        self.rooms
            .iter()
            .all(|room| reachable.contains(&room.center()))
    }
}

impl CollisionMap for DungeonState {
    fn tile_collision_at(&self, tile: TilePos, _grid: &WorldGridConfig) -> TileCollision {
        self.collision_at(tile)
    }
}

// ============================================================================
// 1. Procedural BSP Dungeon Generator
// ============================================================================

/// Configuration parameters for procedural BSP dungeon generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BspDungeonConfig {
    pub width: u32,
    pub height: u32,
    pub max_depth: u32,
    pub min_leaf_size: u32,
    pub min_room_size: u32,
    pub tile_size_px: u32,
}

impl Default for BspDungeonConfig {
    fn default() -> Self {
        Self {
            width: 32,
            height: 24,
            max_depth: 3,
            min_leaf_size: 7,
            min_room_size: 4,
            tile_size_px: DEFAULT_TILE_SIZE_PX,
        }
    }
}

#[derive(Debug, Clone)]
struct BspRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Deterministically generates a procedural dungeon using **Binary Space Partitioning (BSP)**
/// and validates connectivity from `spawn_tile` to `exit_tile` via Flood-Fill.
pub fn generate_bsp_dungeon(
    instance_id: impl Into<String>,
    seed: u64,
    config: &BspDungeonConfig,
) -> Result<DungeonState, DungeonGenError> {
    if config.width < MIN_BSP_DUNGEON_DIMENSION || config.height < MIN_BSP_DUNGEON_DIMENSION {
        return Err(DungeonGenError::DimensionsTooSmall {
            width: config.width,
            height: config.height,
        });
    }
    if config.tile_size_px == 0 {
        return Err(DungeonGenError::InvalidTileSize(0));
    }

    let total_tiles = (config.width * config.height) as usize;
    let mut tiles = vec![InstanceTile::solid_wall(WallKind::CliffRock); total_tiles];

    let mut rng_state = seed ^ 0xA5A5_5A5A_F0F0_0F0F;
    let mut next_u64 = || -> u64 {
        rng_state = splitmix64(rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15));
        rng_state
    };

    // Interior region leaves 1-tile solid outer border around the entire map
    let root_rect = BspRect {
        x: 1,
        y: 1,
        w: config.width - 2,
        h: config.height - 2,
    };

    let min_leaf = config.min_leaf_size.max(5);
    let min_room = config.min_room_size.clamp(3, min_leaf - 1);

    let mut leaves = vec![root_rect];
    for _depth in 0..config.max_depth {
        let mut next_leaves = Vec::new();
        for rect in leaves {
            let can_split_h = rect.h >= min_leaf * 2;
            let can_split_v = rect.w >= min_leaf * 2;

            if !can_split_h && !can_split_v {
                next_leaves.push(rect);
                continue;
            }

            let split_horizontal = if can_split_h && can_split_v {
                if rect.h > (rect.w * 5) / 4 {
                    true
                } else if rect.w > (rect.h * 5) / 4 {
                    false
                } else {
                    (next_u64() & 1) == 0
                }
            } else {
                can_split_h
            };

            if split_horizontal {
                let extra = rect.h - min_leaf * 2;
                let split_offset = if extra > 0 {
                    (next_u64() % (extra as u64 + 1)) as u32
                } else {
                    0
                };
                let split_h = min_leaf + split_offset;
                next_leaves.push(BspRect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: split_h,
                });
                next_leaves.push(BspRect {
                    x: rect.x,
                    y: rect.y + split_h,
                    w: rect.w,
                    h: rect.h - split_h,
                });
            } else {
                let extra = rect.w - min_leaf * 2;
                let split_offset = if extra > 0 {
                    (next_u64() % (extra as u64 + 1)) as u32
                } else {
                    0
                };
                let split_w = min_leaf + split_offset;
                next_leaves.push(BspRect {
                    x: rect.x,
                    y: rect.y,
                    w: split_w,
                    h: rect.h,
                });
                next_leaves.push(BspRect {
                    x: rect.x + split_w,
                    y: rect.y,
                    w: rect.w - split_w,
                    h: rect.h,
                });
            }
        }
        leaves = next_leaves;
    }

    // Carve a room inside each leaf
    let mut rooms = Vec::with_capacity(leaves.len());
    for leaf in &leaves {
        let avail_w = leaf.w.saturating_sub(2).max(min_room);
        let avail_h = leaf.h.saturating_sub(2).max(min_room);

        let room_w = if avail_w > min_room {
            min_room + (next_u64() % ((avail_w - min_room + 1) as u64)) as u32
        } else {
            avail_w.min(leaf.w)
        };
        let room_h = if avail_h > min_room {
            min_room + (next_u64() % ((avail_h - min_room + 1) as u64)) as u32
        } else {
            avail_h.min(leaf.h)
        };

        let pad_x = leaf.w.saturating_sub(room_w);
        let pad_y = leaf.h.saturating_sub(room_h);
        let offset_x = if pad_x > 1 {
            1 + (next_u64() % ((pad_x - 1) as u64)) as u32
        } else {
            0
        };
        let offset_y = if pad_y > 1 {
            1 + (next_u64() % ((pad_y - 1) as u64)) as u32
        } else {
            0
        };

        let rx = (leaf.x + offset_x).clamp(1, config.width - room_w - 1);
        let ry = (leaf.y + offset_y).clamp(1, config.height - room_h - 1);
        let room = BspRoom::new(rx, ry, room_w, room_h);

        for ty in room.y..(room.y + room.height) {
            for tx in room.x..(room.x + room.width) {
                let idx = (ty * config.width + tx) as usize;
                tiles[idx] = InstanceTile::walkable_floor(TerrainTileKind::LavenderMoss);
            }
        }
        rooms.push(room);
    }

    // Connect rooms sequentially with L-shaped corridors
    for pair in rooms.windows(2) {
        let a = pair[0].center();
        let b = pair[1].center();
        let horiz_first = (next_u64() & 1) == 0;
        carve_l_corridor(&mut tiles, config.width, config.height, a, b, horiz_first);
    }

    // Also connect first and last room across the tree if more than 2 rooms exist
    if rooms.len() > 2 {
        let mid = rooms.len() / 2;
        carve_l_corridor(
            &mut tiles,
            config.width,
            config.height,
            rooms[0].center(),
            rooms[mid].center(),
            true,
        );
    }

    let spawn_tile = rooms.first().map_or(TilePos::new(2, 2), BspRoom::center);
    let mut exit_tile = rooms.last().map_or(
        TilePos::new(config.width as i32 - 3, config.height as i32 - 3),
        BspRoom::center,
    );
    if exit_tile == spawn_tile && rooms[0].width >= 3 {
        exit_tile = TilePos::new(spawn_tile.0 + 1, spawn_tile.1);
    }

    let instance_id_str = instance_id.into();
    let mut props = vec![InstanceProp {
        tile: exit_tile,
        kind: InstancePropKind::ExitPortal {
            label: "Return Ladder".to_string(),
        },
    }];

    if rooms.len() >= 2 {
        let chest_room = &rooms[rooms.len() / 2];
        let chest_pos = chest_room.center();
        if chest_pos != spawn_tile && chest_pos != exit_tile {
            props.push(InstanceProp {
                tile: chest_pos,
                kind: InstancePropKind::TreasureChest {
                    chest_id: format!("{instance_id_str}.chest_0"),
                    loot_item_id: "item.berry_potion".to_string(),
                    opened: false,
                },
            });
        }
    }

    let mut dungeon = DungeonState {
        instance_id: instance_id_str,
        display_name: format!("Crystal Cavern #{:04X}", seed & 0xFFFF),
        source: InstanceSource::ProceduralBsp { seed },
        width: config.width,
        height: config.height,
        tile_size_px: config.tile_size_px,
        tiles,
        spawn_tile,
        exit_tile,
        rooms,
        props,
    };

    if let Some(t) = dungeon.get_tile_mut(spawn_tile) {
        t.is_spawn = true;
        t.collision = TileCollision::None;
        t.wall = None;
    }
    if let Some(t) = dungeon.get_tile_mut(exit_tile) {
        t.is_exit = true;
        t.collision = TileCollision::None;
        t.wall = None;
        t.terrain = TerrainTileKind::DirtPath;
    }

    // Flood-Fill verification & safety corridor repair if needed
    if !dungeon.is_connected_spawn_to_exit() {
        let room_centers: Vec<TilePos> = dungeon.rooms.iter().map(BspRoom::center).collect();
        for center in room_centers {
            carve_l_corridor(
                &mut dungeon.tiles,
                dungeon.width,
                dungeon.height,
                dungeon.spawn_tile,
                center,
                true,
            );
        }
        carve_l_corridor(
            &mut dungeon.tiles,
            dungeon.width,
            dungeon.height,
            dungeon.spawn_tile,
            dungeon.exit_tile,
            true,
        );
    }

    Ok(dungeon)
}

fn carve_l_corridor(
    tiles: &mut [InstanceTile],
    width: u32,
    height: u32,
    from: TilePos,
    to: TilePos,
    horizontal_first: bool,
) {
    let carve_point = |tiles: &mut [InstanceTile], x: i32, y: i32| {
        if x >= 1 && y >= 1 && (x as u32) < width - 1 && (y as u32) < height - 1 {
            let idx = (y as u32 * width + x as u32) as usize;
            if tiles[idx].collision != TileCollision::None {
                tiles[idx] = InstanceTile::walkable_floor(TerrainTileKind::DirtPath);
            }
        }
    };

    if horizontal_first {
        let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
        for x in x0..=x1 {
            carve_point(tiles, x, from.1);
        }
        let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
        for y in y0..=y1 {
            carve_point(tiles, to.0, y);
        }
    } else {
        let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
        for y in y0..=y1 {
            carve_point(tiles, from.0, y);
        }
        let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
        for x in x0..=x1 {
            carve_point(tiles, x, to.1);
        }
    }
}

// ============================================================================
// 2. Static Prefab Definition & Loader (RON / JSON / ASCII)
// ============================================================================

/// Data-driven definition for a static room/house prefab loaded from RON or JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefabDefinition {
    pub id: String,
    pub display_name: String,
    pub width: u32,
    pub height: u32,
    /// Row-by-row ASCII representation of the prefab (`height` strings of `width` chars each).
    pub rows: Vec<String>,
    /// Optional mapping from ASCII symbol to a registered `TileDefinition` ID.
    #[serde(default)]
    pub legend: BTreeMap<char, String>,
    /// Optional explicit spawn point override. If `None`, inferred from `'S'` or `'P'` in `rows`.
    #[serde(default)]
    pub spawn_point: Option<TilePos>,
    /// Optional explicit exit point override. If `None`, inferred from `'E'` or `'X'` in `rows`.
    #[serde(default)]
    pub exit_point: Option<TilePos>,
}

impl PrefabDefinition {
    /// Built-in fallback starter cottage prefab guaranteed to be valid even if external files are missing/corrupt.
    #[must_use]
    pub fn starter_cottage_prefab() -> Self {
        let rows = vec![
            "############".to_string(),
            "#..D....C..#".to_string(),
            "#..........#".to_string(),
            "#....N.....#".to_string(),
            "#..........#".to_string(),
            "#..T....T..#".to_string(),
            "#..........#".to_string(),
            "#....S.....#".to_string(),
            "#....E.....#".to_string(),
            "############".to_string(),
        ];
        let mut legend = BTreeMap::new();
        legend.insert('#', "tile.pastel_cobble".to_string());
        legend.insert('.', "tile.pastel_path".to_string());
        legend.insert('S', "tile.pastel_path".to_string());
        legend.insert('E', "tile.buttercup_sand".to_string());
        legend.insert('C', "tile.pastel_path".to_string());
        legend.insert('N', "tile.pastel_path".to_string());
        legend.insert('D', "tile.lavender_floor".to_string());
        legend.insert('T', "tile.pastel_cobble".to_string());

        Self {
            id: "prefab.starter_house".to_string(),
            display_name: "Cozy Pastel Cottage".to_string(),
            width: 12,
            height: 10,
            rows,
            legend,
            spawn_point: None,
            exit_point: None,
        }
    }

    /// Validates this [`PrefabDefinition`] and converts it into a ready-to-play [`DungeonState`].
    pub fn build_instance(
        &self,
        registries: Option<&GameContentRegistries>,
        source_path: Option<String>,
    ) -> Result<DungeonState, PrefabError> {
        if self.id.trim().is_empty() {
            return Err(PrefabError::EmptyId);
        }
        if self.display_name.trim().is_empty() {
            return Err(PrefabError::EmptyDisplayName {
                id: self.id.clone(),
            });
        }
        if self.width < 3 || self.height < 3 {
            return Err(PrefabError::InvalidDimensions {
                id: self.id.clone(),
                width: self.width,
                height: self.height,
            });
        }
        if self.rows.len() != self.height as usize {
            return Err(PrefabError::RowCountMismatch {
                id: self.id.clone(),
                expected_height: self.height,
                actual_rows: self.rows.len(),
            });
        }

        // Validate all legend tile IDs against known/registered tile IDs
        for tile_id in self.legend.values() {
            let known_builtin = BUILTIN_VALID_TILE_IDS.contains(&tile_id.as_str());
            let known_registry = registries.is_some_and(|r| r.tiles.get(tile_id).is_some());
            if !known_builtin && !known_registry {
                return Err(PrefabError::UnknownTileId {
                    id: self.id.clone(),
                    tile_id: tile_id.clone(),
                });
            }
        }

        let mut tiles = Vec::with_capacity((self.width * self.height) as usize);
        let mut inferred_spawn: Option<TilePos> = None;
        let mut inferred_exit: Option<TilePos> = None;
        let mut props = Vec::new();

        for (ry, row_str) in self.rows.iter().enumerate() {
            let chars: Vec<char> = row_str.chars().collect();
            if chars.len() != self.width as usize {
                return Err(PrefabError::ColumnCountMismatch {
                    id: self.id.clone(),
                    row: ry,
                    expected_width: self.width,
                    actual_width: chars.len(),
                });
            }

            for (rx, ch) in chars.into_iter().enumerate() {
                let pos = TilePos::new(rx as i32, ry as i32);
                let mut cell = match ch {
                    '#' => InstanceTile::solid_wall(WallKind::CobbleWall),
                    'T' => InstanceTile::solid_wall(WallKind::AncientPillar),
                    '~' => InstanceTile::water(),
                    '.' => InstanceTile::walkable_floor(TerrainTileKind::DirtPath),
                    'S' | 'P' => {
                        inferred_spawn = Some(pos);
                        let mut t = InstanceTile::walkable_floor(TerrainTileKind::DirtPath);
                        t.is_spawn = true;
                        t
                    }
                    'E' | 'X' => {
                        inferred_exit = Some(pos);
                        props.push(InstanceProp {
                            tile: pos,
                            kind: InstancePropKind::ExitPortal {
                                label: "Door to Overworld".to_string(),
                            },
                        });
                        let mut t = InstanceTile::walkable_floor(TerrainTileKind::SandBank);
                        t.is_exit = true;
                        t
                    }
                    'C' => {
                        props.push(InstanceProp {
                            tile: pos,
                            kind: InstancePropKind::TreasureChest {
                                chest_id: format!("{}.chest_{}_{}", self.id, rx, ry),
                                loot_item_id: "item.pastel_sword".to_string(),
                                opened: false,
                            },
                        });
                        InstanceTile::walkable_floor(TerrainTileKind::DirtPath)
                    }
                    'N' => {
                        props.push(InstanceProp {
                            tile: pos,
                            kind: InstancePropKind::Npc {
                                npc_id: format!("{}.npc_{}_{}", self.id, rx, ry),
                                display_name: "Caretaker Pixl".to_string(),
                                dialogue: "Welcome to our cozy sanctuary!".to_string(),
                            },
                        });
                        InstanceTile::walkable_floor(TerrainTileKind::DirtPath)
                    }
                    'D' => {
                        props.push(InstanceProp {
                            tile: pos,
                            kind: InstancePropKind::Decoration {
                                id: "deco.lavender_rug".to_string(),
                            },
                        });
                        InstanceTile::walkable_floor(TerrainTileKind::LavenderMoss)
                    }
                    other => {
                        if let Some(mapped_id) = self.legend.get(&other) {
                            tile_from_id(mapped_id)
                        } else {
                            return Err(PrefabError::UnknownTileSymbol {
                                id: self.id.clone(),
                                symbol: other,
                                x: rx as u32,
                                y: ry as u32,
                            });
                        }
                    }
                };

                if let Some(mapped_id) = self.legend.get(&ch) {
                    cell.tile_id.clone_from(mapped_id);
                }

                tiles.push(cell);
            }
        }

        let spawn_tile =
            self.spawn_point
                .or(inferred_spawn)
                .ok_or_else(|| PrefabError::MissingSpawnPoint {
                    id: self.id.clone(),
                })?;

        let exit_tile = self.exit_point.or(inferred_exit).unwrap_or(spawn_tile);

        let mut state = DungeonState {
            instance_id: self.id.clone(),
            display_name: self.display_name.clone(),
            source: InstanceSource::Prefab {
                prefab_id: self.id.clone(),
                source_path,
            },
            width: self.width,
            height: self.height,
            tile_size_px: DEFAULT_TILE_SIZE_PX,
            tiles,
            spawn_tile,
            exit_tile,
            rooms: vec![BspRoom::new(1, 1, self.width - 2, self.height - 2)],
            props,
        };

        if !state.collision_at(spawn_tile).is_walkable() {
            return Err(PrefabError::InvalidSpawnPoint {
                id: self.id.clone(),
                x: spawn_tile.0,
                y: spawn_tile.1,
            });
        }
        if !state.collision_at(exit_tile).is_walkable() {
            return Err(PrefabError::InvalidExitPoint {
                id: self.id.clone(),
                x: exit_tile.0,
                y: exit_tile.1,
            });
        }

        if let Some(t) = state.get_tile_mut(spawn_tile) {
            t.is_spawn = true;
        }
        if let Some(t) = state.get_tile_mut(exit_tile) {
            t.is_exit = true;
        }

        Ok(state)
    }
}

fn tile_from_id(tile_id: &str) -> InstanceTile {
    match tile_id {
        "tile.pastel_cobble" => InstanceTile::solid_wall(WallKind::CobbleWall),
        "tile.deep_water" => InstanceTile::water(),
        "tile.lavender_floor" => InstanceTile::walkable_floor(TerrainTileKind::LavenderMoss),
        "tile.buttercup_sand" => InstanceTile::walkable_floor(TerrainTileKind::SandBank),
        "tile.pastel_meadow" => InstanceTile::walkable_floor(TerrainTileKind::MeadowGrass),
        _ => InstanceTile::walkable_floor(TerrainTileKind::DirtPath),
    }
}

/// Stateless loader for deserializing [`PrefabDefinition`]s from RON, JSON, or files on disk.
pub struct PrefabLoader;

impl PrefabLoader {
    /// Parses a [`PrefabDefinition`] from a RON or JSON string and validates it into a [`DungeonState`].
    pub fn load_from_str(
        raw: &str,
        format: DataFormat,
        registries: Option<&GameContentRegistries>,
    ) -> Result<DungeonState, PrefabError> {
        let def = Self::deserialize_definition(raw, format)?;
        def.build_instance(registries, None)
    }

    /// Deserializes a raw [`PrefabDefinition`] without building the instance yet.
    pub fn deserialize_definition(
        raw: &str,
        format: DataFormat,
    ) -> Result<PrefabDefinition, PrefabError> {
        match format {
            DataFormat::Ron => {
                #[cfg(feature = "ron-content")]
                {
                    ron::from_str(raw).map_err(|e| PrefabError::RonParse(e.to_string()))
                }
                #[cfg(not(feature = "ron-content"))]
                {
                    let _ = raw;
                    Err(PrefabError::UnsupportedFormat("ron".to_string()))
                }
            }
            DataFormat::Json => {
                #[cfg(feature = "json-content")]
                {
                    serde_json::from_str(raw).map_err(|e| PrefabError::JsonParse(e.to_string()))
                }
                #[cfg(not(feature = "json-content"))]
                {
                    let _ = raw;
                    Err(PrefabError::UnsupportedFormat("json".to_string()))
                }
            }
        }
    }

    /// Loads and validates a prefab from a `.ron` or `.json` file on disk.
    pub fn load_from_file(
        path: impl AsRef<Path>,
        registries: Option<&GameContentRegistries>,
    ) -> Result<DungeonState, PrefabError> {
        let path_ref = path.as_ref();
        let format = DataFormat::from_path(path_ref)
            .map_err(|e| PrefabError::UnsupportedFormat(e.to_string()))?;
        let raw = fs::read_to_string(path_ref)?;
        let def = Self::deserialize_definition(&raw, format)?;
        def.build_instance(registries, Some(path_ref.display().to_string()))
    }

    /// Loads a prefab from `path`, falling back deterministically to the built-in starter cottage
    /// if the file is missing or corrupted (preventing any runtime panics).
    #[must_use]
    pub fn load_from_file_or_fallback(
        path: impl AsRef<Path>,
        registries: Option<&GameContentRegistries>,
    ) -> DungeonState {
        match Self::load_from_file(path.as_ref(), registries) {
            Ok(instance) => instance,
            Err(err) => {
                log::warn!(
                    "Failed to load prefab '{}': {err}. Using built-in starter cottage fallback.",
                    path.as_ref().display()
                );
                PrefabDefinition::starter_cottage_prefab()
                    .build_instance(None, None)
                    .unwrap_or_else(|_| DungeonState {
                        instance_id: "prefab.fallback".to_string(),
                        display_name: "Fallback Room".to_string(),
                        source: InstanceSource::Prefab {
                            prefab_id: "prefab.fallback".to_string(),
                            source_path: None,
                        },
                        width: 5,
                        height: 5,
                        tile_size_px: DEFAULT_TILE_SIZE_PX,
                        tiles: vec![InstanceTile::walkable_floor(TerrainTileKind::DirtPath); 25],
                        spawn_tile: TilePos::new(2, 2),
                        exit_tile: TilePos::new(2, 3),
                        rooms: vec![BspRoom::new(1, 1, 3, 3)],
                        props: Vec::new(),
                    })
            }
        }
    }
}

// ============================================================================
// 3. Scene & SceneManager State Machine
// ============================================================================

/// Active world context managed by [`SceneManager`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scene {
    /// Persistent infinite chunk-streamed Overworld.
    Overworld(OverworldState),
    /// Instanced interior map (Procedural BSP Dungeon or Static Prefab House).
    Instance(DungeonState),
}

impl Default for Scene {
    fn default() -> Self {
        Self::Overworld(OverworldState::default())
    }
}

/// High-level ECS events requesting a transition between the Overworld and an Instance.
#[derive(Event, Debug, Clone, PartialEq)]
pub enum SceneTransitionEvent {
    /// Generates and enters a procedural BSP dungeon with the given `seed`.
    EnterProceduralDungeon {
        instance_id: String,
        seed: u64,
        config: BspDungeonConfig,
        /// Optional explicit return position in the Overworld. If `None`, uses the player's current position.
        return_position: Option<Vec2>,
    },
    /// Loads and enters a static prefab room from disk (with safe fallback on missing/corrupt files).
    EnterPrefab {
        prefab_path: String,
        /// Optional explicit return position in the Overworld. If `None`, uses the player's current position.
        return_position: Option<Vec2>,
    },
    /// Directly enters a pre-constructed [`DungeonState`] instance.
    EnterInstance {
        instance: DungeonState,
        return_position: Option<Vec2>,
    },
    /// Exits the current instance and restores the player to the saved `return_position` in the Overworld.
    ExitToOverworld,
}

/// Bevy Resource managing the [`Scene`] state machine and the stack of saved [`OverworldState`]s.
#[derive(Resource, Debug, Clone, Default)]
pub struct SceneManager {
    active_scene: Scene,
    overworld_stack: Vec<OverworldState>,
    pub transition_count: u64,
    /// Cooldown ticks after a scene transition to prevent immediate re-triggering on automatic portals.
    pub transition_cooldown_ticks: u32,
}

impl SceneManager {
    /// Creates a new [`SceneManager`] starting in [`Scene::Overworld`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reference to the currently active [`Scene`].
    #[must_use]
    pub const fn active_scene(&self) -> &Scene {
        &self.active_scene
    }

    /// Mutable reference to the currently active [`Scene`].
    pub fn active_scene_mut(&mut self) -> &mut Scene {
        &mut self.active_scene
    }

    /// Returns `true` if the active scene is [`Scene::Overworld`].
    #[must_use]
    pub const fn is_overworld(&self) -> bool {
        matches!(self.active_scene, Scene::Overworld(_))
    }

    /// Returns `true` if the active scene is [`Scene::Instance`].
    #[must_use]
    pub const fn is_instance(&self) -> bool {
        matches!(self.active_scene, Scene::Instance(_))
    }

    /// Returns the active [`DungeonState`] if currently inside an instance.
    #[must_use]
    pub const fn current_instance(&self) -> Option<&DungeonState> {
        match &self.active_scene {
            Scene::Instance(dungeon) => Some(dungeon),
            Scene::Overworld(_) => None,
        }
    }

    /// Returns the most recently saved [`OverworldState`] on the stack (or the active Overworld state).
    #[must_use]
    pub fn saved_overworld(&self) -> Option<&OverworldState> {
        self.overworld_stack.last().or(match &self.active_scene {
            Scene::Overworld(ow) => Some(ow),
            Scene::Instance(_) => None,
        })
    }

    /// Transitions into `instance`, saving `overworld_snapshot` onto the return stack.
    /// Returns the spawn [`WorldPos`] inside the new instance.
    pub fn enter_instance(
        &mut self,
        overworld_snapshot: OverworldState,
        instance: DungeonState,
    ) -> WorldPos {
        let spawn = instance.spawn_world_pos();
        self.overworld_stack.push(overworld_snapshot);
        self.active_scene = Scene::Instance(instance);
        self.transition_count += 1;
        self.transition_cooldown_ticks = 2;
        spawn
    }

    /// Transitions back from an instance to the saved [`OverworldState`].
    /// Returns the restored [`OverworldState`] (including exact `return_position`).
    pub fn exit_to_overworld(&mut self) -> Option<OverworldState> {
        if !self.is_instance() {
            return None;
        }
        let restored = self.overworld_stack.pop().unwrap_or_default();
        self.active_scene = Scene::Overworld(restored.clone());
        self.transition_count += 1;
        self.transition_cooldown_ticks = 2;
        Some(restored)
    }
}

/// Renders an active [`DungeonState`] instance tilemap and its interior props onto the native [`PixelCanvas`].
pub fn render_instance_to_canvas(
    canvas: &mut PixelCanvas,
    palette: &PastelGameBoyPalette,
    camera: &PixelPerfectCamera2d,
    dungeon: &DungeonState,
) {
    let lookup = |idx: u8| -> RgbaColor {
        palette
            .lookup(idx % (palette.len() as u8))
            .unwrap_or(RgbaColor::BLACK)
    };

    let tile_px = dungeon.tile_size_px;
    let ink_color = lookup(3);

    for ty in 0..dungeon.height as i32 {
        for tx in 0..dungeon.width as i32 {
            let pos = TilePos::new(tx, ty);
            let Some(cell) = dungeon.get_tile(pos) else {
                continue;
            };
            let world_px = Vec2::new((tx * tile_px as i32) as f32, (ty * tile_px as i32) as f32);
            let screen_px = camera.world_to_screen_pixel(world_px);

            if screen_px.x + (tile_px as i32) < 0
                || screen_px.y + (tile_px as i32) < 0
                || screen_px.x >= canvas.native_width() as i32
                || screen_px.y >= canvas.native_height() as i32
            {
                continue;
            }

            let base_color = match cell.terrain {
                TerrainTileKind::LavenderMoss => lookup(5),
                TerrainTileKind::DirtPath => lookup(0),
                TerrainTileKind::SandBank => lookup(6),
                TerrainTileKind::DeepWater | TerrainTileKind::ShallowWater => lookup(7),
                _ => lookup(1),
            };

            for py in 0..tile_px {
                for px in 0..tile_px {
                    let mut color = base_color;
                    if let Some(wall) = cell.wall {
                        let is_border =
                            px == 0 || py == 0 || px == tile_px - 1 || py == tile_px - 1;
                        let is_brick = py == tile_px / 2;
                        color = if is_border || is_brick {
                            ink_color
                        } else {
                            match wall {
                                WallKind::CobbleWall => lookup(5),
                                WallKind::CliffRock => lookup(2),
                                WallKind::AncientPillar => lookup(6),
                            }
                        };
                    } else if cell.is_exit
                        && (px >= 4 && px < tile_px - 4 && py >= 4 && py < tile_px - 4)
                    {
                        // Highlight exit portal/mat with Buttercup Sun / Sky Breeze
                        color = if (px + py).is_multiple_of(2) {
                            lookup(6)
                        } else {
                            lookup(7)
                        };
                    }
                    canvas.set_pixel(screen_px.x + px as i32, screen_px.y + py as i32, color);
                }
            }
        }
    }

    // Render interior props (chests, NPCs, decorations)
    for prop in &dungeon.props {
        let world_px = Vec2::new(
            (prop.tile.0 * tile_px as i32) as f32,
            (prop.tile.1 * tile_px as i32) as f32,
        );
        let screen_px = camera.world_to_screen_pixel(world_px);
        let margin = (tile_px / 4) as i32;
        let span = tile_px / 2;
        let prop_color = match &prop.kind {
            InstancePropKind::ExitPortal { .. } => lookup(6),
            InstancePropKind::TreasureChest { opened, .. } => {
                if *opened {
                    lookup(2)
                } else {
                    lookup(6)
                }
            }
            InstancePropKind::Npc { .. } => lookup(4),
            InstancePropKind::Decoration { .. } => lookup(5),
        };
        canvas.fill_rect(
            screen_px.x + margin,
            screen_px.y + margin,
            span,
            span,
            prop_color,
        );
    }
}
