//! Player Controller, Sub-Tile Movement, AABB Sliding Collision & Y-Sorting (Phase 3).
//!
//! Provides:
//! - [`PlayerState`] state machine (`Idle`, `Moving`, `Interacting`, `Transitioning`).
//! - 8-way top-down movement with diagonal speed normalization ([`compute_normalized_movement_delta`]).
//! - Relative sub-tile [`PlayerHitbox`] and [`Aabb`] collision evaluation across chunk borders.
//! - Axis-separated sliding collision with anti-tunneling sub-stepping ([`resolve_subtile_movement`]).
//! - Y-Sorting ([`YSort`], [`compute_y_sort_order`]) so the player renders behind southern walls/objects.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::palette_sprite::{PixelSprite, WorldPixelPos};
use crate::world::coords::{LocalTileCoord, TilePos, WorldGridConfig, WorldPos};
use crate::world::generation::{ChunkData, TileCollision};
use crate::world::streaming::WorldMap;

/// Maximum sub-step displacement (in pixels) per collision iteration.
/// Half of the default `16px` tile size so high-speed or low-FPS deltas never tunnel through 1-tile walls.
pub const MAX_COLLISION_SUBSTEP_PX: f32 = 8.0;

/// Small epsilon subtracted from the upper-right AABB corner when mapping `[min, max)` to grid tiles.
const AABB_MAX_EPSILON: f32 = 1e-4;

/// State machine for the playable character (Phase 3).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PlayerState {
    /// Standing still, ready to move or interact.
    #[default]
    Idle,
    /// Actively moving across the tilemap.
    Moving,
    /// Interacting with an NPC, chest, or object.
    Interacting,
    /// Transitioning between Overworld and an instanced scene (Dungeon / Prefab).
    Transitioning,
}

impl PlayerState {
    /// Returns `true` if the player is allowed to process directional movement in this state.
    #[must_use]
    pub const fn can_move(self) -> bool {
        matches!(self, Self::Idle | Self::Moving | Self::Interacting)
    }
}

/// 8-way facing direction of the player or an entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FacingDirection {
    North,
    NorthEast,
    East,
    SouthEast,
    #[default]
    South,
    SouthWest,
    West,
    NorthWest,
}

impl FacingDirection {
    /// Converts a non-zero movement input or velocity vector into an 8-way [`FacingDirection`].
    /// In screen/world pixel space, `-Y` is North (up) and `+Y` is South (down).
    #[must_use]
    pub fn from_vec2(dir: Vec2) -> Option<Self> {
        let eps = 1e-4;
        let dx = if dir.x > eps {
            1
        } else if dir.x < -eps {
            -1
        } else {
            0
        };
        let dy = if dir.y > eps {
            1
        } else if dir.y < -eps {
            -1
        } else {
            0
        };

        match (dx, dy) {
            (0, -1) => Some(Self::North),
            (1, -1) => Some(Self::NorthEast),
            (1, 0) => Some(Self::East),
            (1, 1) => Some(Self::SouthEast),
            (0, 1) => Some(Self::South),
            (-1, 1) => Some(Self::SouthWest),
            (-1, 0) => Some(Self::West),
            (-1, -1) => Some(Self::NorthWest),
            _ => None,
        }
    }

    /// Normalized 2D unit direction vector for this facing direction.
    #[must_use]
    pub fn unit_vector(self) -> Vec2 {
        let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
        match self {
            Self::North => Vec2::new(0.0, -1.0),
            Self::NorthEast => Vec2::new(inv_sqrt2, -inv_sqrt2),
            Self::East => Vec2::new(1.0, 0.0),
            Self::SouthEast => Vec2::new(inv_sqrt2, inv_sqrt2),
            Self::South => Vec2::new(0.0, 1.0),
            Self::SouthWest => Vec2::new(-inv_sqrt2, inv_sqrt2),
            Self::West => Vec2::new(-1.0, 0.0),
            Self::NorthWest => Vec2::new(-inv_sqrt2, -inv_sqrt2),
        }
    }

    /// Discrete tile step offset `(dx, dy)` in the facing direction.
    #[must_use]
    pub const fn tile_offset(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::NorthEast => (1, -1),
            Self::East => (1, 0),
            Self::SouthEast => (1, 1),
            Self::South => (0, 1),
            Self::SouthWest => (-1, 1),
            Self::West => (-1, 0),
            Self::NorthWest => (-1, -1),
        }
    }
}

/// Axis-Aligned Bounding Box in continuous world pixel coordinates (`[min, max)`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    /// Creates a new [`Aabb`] from `min` and `max` corners.
    #[must_use]
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self {
            min: min.min(max),
            max: min.max(max),
        }
    }

    /// Creates an [`Aabb`] from a top-left origin and size.
    #[must_use]
    pub fn from_origin_size(origin: Vec2, size: Vec2) -> Self {
        let clamped_size = size.max(Vec2::splat(0.1));
        Self {
            min: origin,
            max: origin + clamped_size,
        }
    }

    /// Width and height of the bounding box.
    #[must_use]
    pub fn size(self) -> Vec2 {
        self.max - self.min
    }

    /// Center point of the bounding box in world pixel space.
    #[must_use]
    pub fn center(self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// Returns a new [`Aabb`] translated by `delta`.
    #[must_use]
    pub fn translated(self, delta: Vec2) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    /// Returns `true` if `self` and `other` overlap with positive area.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }
}

/// Player hitbox component defined as a relative offset and size smaller than a full `16x16` tile
/// (typically covering the lower feet/torso area of the player sprite for natural top-down perspective).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerHitbox {
    /// Offset from the entity's [`WorldPos`] / [`WorldPixelPos`] to the top-left of the hitbox.
    pub offset: Vec2,
    /// Width and height of the hitbox in pixels (smaller than tile size `16x16`).
    pub size: Vec2,
}

impl Default for PlayerHitbox {
    fn default() -> Self {
        // Default 6x4 lower-body hitbox at offset (1.0, 3.0) inside the 8x8 Hero Pixl sprite
        Self {
            offset: Vec2::new(1.0, 3.0),
            size: Vec2::new(6.0, 4.0),
        }
    }
}

impl PlayerHitbox {
    /// Creates a custom [`PlayerHitbox`] with relative `offset` and `size`.
    #[must_use]
    pub const fn new(offset: Vec2, size: Vec2) -> Self {
        Self { offset, size }
    }

    /// Computes the world-space [`Aabb`] for an entity located at `pos`.
    #[must_use]
    pub fn aabb_at(&self, pos: WorldPos) -> Aabb {
        Aabb::from_origin_size(pos.to_vec2() + self.offset, self.size)
    }

    /// Computes the center of the hitbox in world pixel coordinates when the entity is at `pos`.
    #[must_use]
    pub fn center_at(&self, pos: WorldPos) -> Vec2 {
        self.aabb_at(pos).center()
    }
}

/// Component storing player movement parameters and current velocity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerMovement {
    /// Base movement speed in pixels per second.
    pub speed_px_per_sec: f32,
    /// Last raw directional input vector.
    pub input_dir: Vec2,
    /// Actual displacement vector applied during the most recent movement step.
    pub last_applied_delta: Vec2,
}

impl Default for PlayerMovement {
    fn default() -> Self {
        Self {
            speed_px_per_sec: 64.0,
            input_dir: Vec2::ZERO,
            last_applied_delta: Vec2::ZERO,
        }
    }
}

impl PlayerMovement {
    #[must_use]
    pub const fn new(speed_px_per_sec: f32) -> Self {
        Self {
            speed_px_per_sec,
            input_dir: Vec2::ZERO,
            last_applied_delta: Vec2::ZERO,
        }
    }
}

/// Resource tracking continuous keyboard/controller directional input and interaction triggers.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerInputState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    /// Whether the interaction key (`E` or `Space`) was pressed on this frame.
    pub interact_just_pressed: bool,
    /// Frame delta time in seconds used for continuous movement steps (default: `1.0 / 60.0`).
    pub delta_time: f32,
}

impl Default for PlayerInputState {
    fn default() -> Self {
        Self {
            up: false,
            down: false,
            left: false,
            right: false,
            interact_just_pressed: false,
            delta_time: 1.0 / 60.0,
        }
    }
}

impl PlayerInputState {
    /// Computes the raw 8-way input vector from currently pressed directional flags.
    #[must_use]
    pub fn raw_direction(&self) -> Vec2 {
        let mut dir = Vec2::ZERO;
        if self.left {
            dir.x -= 1.0;
        }
        if self.right {
            dir.x += 1.0;
        }
        if self.up {
            dir.y -= 1.0;
        }
        if self.down {
            dir.y += 1.0;
        }
        dir
    }

    /// Clears all pressed directional and interaction flags.
    pub fn clear(&mut self) {
        self.up = false;
        self.down = false;
        self.left = false;
        self.right = false;
        self.interact_just_pressed = false;
    }
}

/// Component for Y-Sorted rendering order (`z_order = base_layer + (world_y + offset_y).round()`).
/// Ensures that entities with a lower screen Y (further north) render behind walls and objects
/// with a higher screen Y (further south).
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct YSort {
    /// Vertical pixel offset from the entity's `WorldPixelPos.y` to its sorting anchor (typically bottom edge / feet).
    pub offset_y: f32,
    /// Base layer offset added to the Y coordinate.
    pub base_layer: i32,
}

impl Default for YSort {
    fn default() -> Self {
        Self {
            offset_y: 8.0,
            base_layer: 100,
        }
    }
}

impl YSort {
    #[must_use]
    pub const fn new(offset_y: f32, base_layer: i32) -> Self {
        Self {
            offset_y,
            base_layer,
        }
    }
}

/// Pure function computing the deterministic Z-order from an entity's [`WorldPos`] and [`YSort`] parameters.
#[must_use]
pub fn compute_y_sort_order(pos: WorldPos, offset_y: f32, base_layer: i32) -> i32 {
    base_layer.saturating_add((pos.1 + offset_y).round() as i32)
}

/// Trait abstracting tile collision queries across Overworld chunk maps, single chunks,
/// instanced dungeons/prefabs, and standalone test grids.
///
/// Out-of-bounds or unloaded tile lookups MUST deterministically return [`TileCollision::Solid`]
/// without panicking.
pub trait CollisionMap {
    /// Returns the [`TileCollision`] state at global or instance [`TilePos`].
    fn tile_collision_at(&self, tile: TilePos, grid: &WorldGridConfig) -> TileCollision;
}

impl CollisionMap for WorldMap {
    fn tile_collision_at(&self, tile: TilePos, grid: &WorldGridConfig) -> TileCollision {
        self.get_tile(tile.to_tile_coord(), grid)
            .map_or(TileCollision::Solid, |cell| cell.collision())
    }
}

impl CollisionMap for ChunkData {
    fn tile_collision_at(&self, tile: TilePos, grid: &WorldGridConfig) -> TileCollision {
        let (chunk_coord, local) = grid.tile_to_chunk_and_local(tile.to_tile_coord());
        if chunk_coord != self.coord {
            return TileCollision::Solid;
        }
        self.get_tile(local)
            .map_or(TileCollision::Solid, |cell| cell.collision())
    }
}

/// Standalone deterministic collision grid useful for unit tests and custom collision layouts.
#[derive(Debug, Clone, Default)]
pub struct FixedCollisionGrid {
    pub width: i32,
    pub height: i32,
    pub default_in_bounds: TileCollision,
    pub overrides: BTreeMap<TilePos, TileCollision>,
}

impl FixedCollisionGrid {
    /// Creates a bounded `width x height` walkable grid (`[0..width, 0..height]`) where out-of-bounds tiles are `Solid`.
    #[must_use]
    pub fn new_walkable(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            default_in_bounds: TileCollision::None,
            overrides: BTreeMap::new(),
        }
    }

    /// Sets the [`TileCollision`] for a specific [`TilePos`].
    pub fn set_collision(&mut self, pos: TilePos, collision: TileCollision) {
        self.overrides.insert(pos, collision);
    }
}

impl CollisionMap for FixedCollisionGrid {
    fn tile_collision_at(&self, tile: TilePos, _grid: &WorldGridConfig) -> TileCollision {
        if tile.0 < 0 || tile.1 < 0 || tile.0 >= self.width || tile.1 >= self.height {
            return TileCollision::Solid;
        }
        self.overrides
            .get(&tile)
            .copied()
            .unwrap_or(self.default_in_bounds)
    }
}

impl<F> CollisionMap for F
where
    F: Fn(TilePos) -> TileCollision,
{
    fn tile_collision_at(&self, tile: TilePos, _grid: &WorldGridConfig) -> TileCollision {
        (self)(tile)
    }
}

/// Pure function that computes the 8-way top-down movement displacement vector
/// (`normalized_dir * speed * delta_time`).
///
/// Guarantees that diagonal movement has the exact same Euclidean speed as cardinal movement
/// (not $\sqrt{2} \times$ faster).
#[must_use]
pub fn compute_normalized_movement_delta(input_dir: Vec2, speed: f32, delta_time: f32) -> Vec2 {
    if speed <= 0.0 || delta_time <= 0.0 || !speed.is_finite() || !delta_time.is_finite() {
        return Vec2::ZERO;
    }
    let normalized = input_dir.normalize_or_zero();
    normalized * (speed * delta_time)
}

/// Pure function that calculates all grid [`TilePos`] coordinates overlapped by `aabb`.
#[must_use]
pub fn overlapped_tiles_for_aabb(aabb: Aabb, grid: &WorldGridConfig) -> Vec<TilePos> {
    let min_tile = grid.world_pixel_to_tile(aabb.min);
    let max_sample = Vec2::new(
        (aabb.max.x - AABB_MAX_EPSILON).max(aabb.min.x),
        (aabb.max.y - AABB_MAX_EPSILON).max(aabb.min.y),
    );
    let max_tile = grid.world_pixel_to_tile(max_sample);

    let cap =
        ((max_tile.x - min_tile.x + 1).max(1) * (max_tile.y - min_tile.y + 1).max(1)) as usize;
    let mut tiles = Vec::with_capacity(cap);

    for ty in min_tile.y..=max_tile.y {
        for tx in min_tile.x..=max_tile.x {
            tiles.push(TilePos::new(tx, ty));
        }
    }
    tiles
}

/// Pure function that returns `true` if `aabb` overlaps any blocking tile (`TileCollision::Solid` or `TileCollision::Water`)
/// in `map`.
#[must_use]
pub fn check_aabb_collision<M: CollisionMap + ?Sized>(
    aabb: Aabb,
    map: &M,
    grid: &WorldGridConfig,
) -> bool {
    let min_tile = grid.world_pixel_to_tile(aabb.min);
    let max_sample = Vec2::new(
        (aabb.max.x - AABB_MAX_EPSILON).max(aabb.min.x),
        (aabb.max.y - AABB_MAX_EPSILON).max(aabb.min.y),
    );
    let max_tile = grid.world_pixel_to_tile(max_sample);

    for ty in min_tile.y..=max_tile.y {
        for tx in min_tile.x..=max_tile.x {
            let collision = map.tile_collision_at(TilePos::new(tx, ty), grid);
            if collision.is_blocking() {
                return true;
            }
        }
    }
    false
}

/// Pure function that resolves sub-tile movement with **X/Y axis-separated sliding collision**
/// and **anti-tunneling sub-stepping**.
///
/// - Separates X and Y movement checks so diagonal movement against a vertical or horizontal wall
///   slides smoothly along the unblocked axis instead of getting stuck on corners.
/// - Sub-steps large displacement vectors (`> MAX_COLLISION_SUBSTEP_PX`) so high speeds or low
///   framerates can never push the AABB through a 1-tile wall.
///
/// Returns `(new_world_pos, actual_applied_delta)`.
#[must_use]
pub fn resolve_subtile_movement<M: CollisionMap + ?Sized>(
    current_pos: WorldPos,
    hitbox: &PlayerHitbox,
    delta: Vec2,
    map: &M,
    grid: &WorldGridConfig,
) -> (WorldPos, Vec2) {
    if delta == Vec2::ZERO || !delta.x.is_finite() || !delta.y.is_finite() {
        return (current_pos, Vec2::ZERO);
    }

    let max_axis = delta.x.abs().max(delta.y.abs());
    let max_step = MAX_COLLISION_SUBSTEP_PX
        .min(grid.tile_size_px as f32 * 0.5)
        .max(1.0);
    let steps = (max_axis / max_step).ceil().max(1.0) as usize;
    let step_delta = delta / (steps as f32);

    let mut pos = current_pos.to_vec2();
    let mut x_blocked = false;
    let mut y_blocked = false;

    for _ in 0..steps {
        // 1. Resolve X-axis independently
        if !x_blocked && step_delta.x != 0.0 {
            let candidate_x = WorldPos::new(pos.x + step_delta.x, pos.y);
            let aabb_x = hitbox.aabb_at(candidate_x);
            if !check_aabb_collision(aabb_x, map, grid) {
                pos.x = candidate_x.0;
            } else {
                x_blocked = true;
            }
        }

        // 2. Resolve Y-axis independently (using the updated X position)
        if !y_blocked && step_delta.y != 0.0 {
            let candidate_y = WorldPos::new(pos.x, pos.y + step_delta.y);
            let aabb_y = hitbox.aabb_at(candidate_y);
            if !check_aabb_collision(aabb_y, map, grid) {
                pos.y = candidate_y.1;
            } else {
                y_blocked = true;
            }
        }

        if x_blocked && y_blocked {
            break;
        }
    }

    let final_pos = WorldPos::from(pos);
    let applied_delta = pos - current_pos.to_vec2();
    (final_pos, applied_delta)
}

/// ECS system that updates `PixelSprite.z_order` from [`WorldPixelPos`] and [`YSort`]
/// so sprites and foreground objects are Y-sorted deterministically.
pub fn update_y_sorting_system(mut query: Query<(&WorldPixelPos, &YSort, &mut PixelSprite)>) {
    for (pos, y_sort, mut sprite) in &mut query {
        sprite.z_order =
            compute_y_sort_order(WorldPos::from(*pos), y_sort.offset_y, y_sort.base_layer);
    }
}

/// Helper to query a tile across chunk boundaries from a slice or map of [`ChunkData`].
#[must_use]
pub fn sample_collision_across_chunks(
    chunks: &BTreeMap<crate::world::coords::ChunkCoord, ChunkData>,
    tile: TilePos,
    grid: &WorldGridConfig,
) -> TileCollision {
    let (chunk_coord, local): (crate::world::coords::ChunkCoord, LocalTileCoord) =
        grid.tile_to_chunk_and_local(tile.to_tile_coord());
    chunks
        .get(&chunk_coord)
        .and_then(|c| c.get_tile(local))
        .map_or(TileCollision::Solid, |cell| cell.collision())
}
