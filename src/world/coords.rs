//! Coordinate and Grid Mathematics for the Tile & Chunk World System (Phase 2).
//!
//! Defines strongly-typed global tile coordinates ([`TileCoord`]), chunk coordinates
//! ([`ChunkCoord`]), chunk-local tile coordinates ([`LocalTileCoord`]), and the
//! [`WorldGridConfig`] resource (defaulting to `16x16` pixels per tile and `16x16` tiles per chunk).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::palette_sprite::WorldPixelPos;

/// Default width and height of a single world tile in retro pixels (`16x16`).
pub const DEFAULT_TILE_SIZE_PX: u32 = 16;

/// Default width and height of a world chunk measured in tiles (`16x16` = 256 tiles per chunk).
pub const DEFAULT_CHUNK_SIZE_TILES: u32 = 16;

/// Errors related to grid configuration or local tile indexing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GridError {
    #[error("Tile size in pixels must be greater than 0, got {0}")]
    InvalidTileSize(u32),
    #[error("Chunk size in tiles must be greater than 0, got {0}")]
    InvalidChunkSize(u32),
    #[error("Local tile coordinate ({x}, {y}) is out of bounds for chunk size {chunk_size}x{chunk_size}")]
    LocalCoordOutOfBounds { x: u32, y: u32, chunk_size: u32 },
}

/// Global 2D tile coordinate on the infinite world grid.
/// Can hold negative, zero, or positive integer coordinates.
#[derive(
    Component,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
)]
pub struct TileCoord {
    pub x: i32,
    pub y: i32,
}

impl TileCoord {
    pub const ZERO: Self = Self { x: 0, y: 0 };

    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn offset(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
        }
    }

    /// Manhattan (`L1`) distance between two global tile coordinates.
    #[must_use]
    pub const fn manhattan_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x) + self.y.abs_diff(other.y)
    }

    /// Chebyshev (`L_infinity`) distance between two global tile coordinates.
    #[must_use]
    pub fn chebyshev_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.y.abs_diff(other.y))
    }

    /// Returns the 4 cardinal neighbor tile coordinates (`North`, `East`, `South`, `West`).
    #[must_use]
    pub const fn cardinal_neighbors(self) -> [Self; 4] {
        [
            Self::new(self.x, self.y - 1),
            Self::new(self.x + 1, self.y),
            Self::new(self.x, self.y + 1),
            Self::new(self.x - 1, self.y),
        ]
    }
}

/// Global 2D chunk coordinate on the infinite world chunk grid.
#[derive(
    Component,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
}

impl ChunkCoord {
    pub const ZERO: Self = Self { x: 0, y: 0 };

    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn offset(self, dx: i32, dy: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
        }
    }

    /// Manhattan (`L1`) distance between two chunk coordinates.
    #[must_use]
    pub const fn manhattan_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x) + self.y.abs_diff(other.y)
    }

    /// Chebyshev (`L_infinity` square ring) distance between two chunk coordinates.
    #[must_use]
    pub fn chebyshev_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.y.abs_diff(other.y))
    }

    /// Returns all chunk coordinates within a square Chebyshev `radius` around `self`,
    /// sorted by distance from `self` (closest chunks first, then deterministic `(y, x)` order).
    #[must_use]
    pub fn chunks_in_radius(self, radius: i32) -> Vec<Self> {
        if radius < 0 {
            return Vec::new();
        }
        let side = (radius * 2 + 1) as usize;
        let mut coords = Vec::with_capacity(side * side);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                coords.push(Self::new(self.x + dx, self.y + dy));
            }
        }
        coords.sort_by_key(|c| {
            let dist_sq = (c.x - self.x) as i64 * (c.x - self.x) as i64
                + (c.y - self.y) as i64 * (c.y - self.y) as i64;
            (dist_sq, c.y, c.x)
        });
        coords
    }
}

/// Chunk-local tile coordinate (`0..chunk_size_tiles` on both axes).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct LocalTileCoord {
    pub x: u32,
    pub y: u32,
}

impl LocalTileCoord {
    pub const ZERO: Self = Self { x: 0, y: 0 };

    #[must_use]
    pub const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

/// Resource defining the world's tile dimensions and chunk partition dimensions.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldGridConfig {
    /// Pixel width and height of a single tile (default: `16`).
    pub tile_size_px: u32,
    /// Number of tiles along each axis of a chunk (default: `16`, yielding `16x16 = 256` tiles).
    pub chunk_size_tiles: u32,
}

impl Default for WorldGridConfig {
    fn default() -> Self {
        Self {
            tile_size_px: DEFAULT_TILE_SIZE_PX,
            chunk_size_tiles: DEFAULT_CHUNK_SIZE_TILES,
        }
    }
}

impl WorldGridConfig {
    /// Creates and validates a custom `WorldGridConfig`.
    pub fn new(tile_size_px: u32, chunk_size_tiles: u32) -> Result<Self, GridError> {
        let cfg = Self {
            tile_size_px,
            chunk_size_tiles,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validates that tile size and chunk size are both non-zero.
    pub fn validate(&self) -> Result<(), GridError> {
        if self.tile_size_px == 0 {
            return Err(GridError::InvalidTileSize(0));
        }
        if self.chunk_size_tiles == 0 {
            return Err(GridError::InvalidChunkSize(0));
        }
        Ok(())
    }

    /// Total number of tiles inside a single chunk (`chunk_size_tiles * chunk_size_tiles`).
    #[must_use]
    pub const fn tiles_per_chunk(&self) -> usize {
        (self.chunk_size_tiles * self.chunk_size_tiles) as usize
    }

    /// Width and height of a full chunk in world pixels (`tile_size_px * chunk_size_tiles`).
    #[must_use]
    pub const fn chunk_size_px(&self) -> u32 {
        self.tile_size_px * self.chunk_size_tiles
    }

    /// Converts continuous world pixel coordinates (`Vec2`) into global [`TileCoord`]
    /// using Euclidean floor division so negative coordinates map without off-by-one errors.
    #[must_use]
    pub fn world_pixel_to_tile(&self, world_px: Vec2) -> TileCoord {
        let ts = self.tile_size_px as i32;
        let px_x = world_px.x.floor() as i32;
        let px_y = world_px.y.floor() as i32;
        TileCoord::new(px_x.div_euclid(ts), px_y.div_euclid(ts))
    }

    /// Returns the top-left world pixel position ([`WorldPixelPos`]) of the given [`TileCoord`].
    #[must_use]
    pub const fn tile_to_world_pixel(&self, tile: TileCoord) -> WorldPixelPos {
        let ts = self.tile_size_px as i32;
        WorldPixelPos::new((tile.x * ts) as f32, (tile.y * ts) as f32)
    }

    /// Returns the center world pixel position ([`WorldPixelPos`]) of the given [`TileCoord`].
    #[must_use]
    pub fn tile_center_world_pixel(&self, tile: TileCoord) -> WorldPixelPos {
        let top_left = self.tile_to_world_pixel(tile);
        let half = self.tile_size_px as f32 * 0.5;
        WorldPixelPos::new(top_left.x + half, top_left.y + half)
    }

    /// Maps a global [`TileCoord`] to the [`ChunkCoord`] that contains it.
    #[must_use]
    pub const fn tile_to_chunk(&self, tile: TileCoord) -> ChunkCoord {
        let cs = self.chunk_size_tiles as i32;
        ChunkCoord::new(tile.x.div_euclid(cs), tile.y.div_euclid(cs))
    }

    /// Maps a global [`TileCoord`] to its chunk-local [`LocalTileCoord`] (`0..chunk_size_tiles`).
    #[must_use]
    pub const fn tile_to_local(&self, tile: TileCoord) -> LocalTileCoord {
        let cs = self.chunk_size_tiles as i32;
        LocalTileCoord::new(tile.x.rem_euclid(cs) as u32, tile.y.rem_euclid(cs) as u32)
    }

    /// Decomposes a global [`TileCoord`] into `(ChunkCoord, LocalTileCoord)`.
    #[must_use]
    pub const fn tile_to_chunk_and_local(&self, tile: TileCoord) -> (ChunkCoord, LocalTileCoord) {
        (self.tile_to_chunk(tile), self.tile_to_local(tile))
    }

    /// Reconstructs a global [`TileCoord`] from a [`ChunkCoord`] and [`LocalTileCoord`].
    #[must_use]
    pub const fn chunk_and_local_to_tile(
        &self,
        chunk: ChunkCoord,
        local: LocalTileCoord,
    ) -> TileCoord {
        let cs = self.chunk_size_tiles as i32;
        TileCoord::new(chunk.x * cs + local.x as i32, chunk.y * cs + local.y as i32)
    }

    /// Converts continuous world pixel coordinates (`Vec2`) directly to the containing [`ChunkCoord`].
    #[must_use]
    pub fn world_pixel_to_chunk(&self, world_px: Vec2) -> ChunkCoord {
        self.tile_to_chunk(self.world_pixel_to_tile(world_px))
    }

    /// Returns the top-left global [`TileCoord`] of the given [`ChunkCoord`].
    #[must_use]
    pub const fn chunk_origin_tile(&self, chunk: ChunkCoord) -> TileCoord {
        self.chunk_and_local_to_tile(chunk, LocalTileCoord::ZERO)
    }

    /// Returns the top-left world pixel position ([`WorldPixelPos`]) of the given [`ChunkCoord`].
    #[must_use]
    pub const fn chunk_origin_world_pixel(&self, chunk: ChunkCoord) -> WorldPixelPos {
        self.tile_to_world_pixel(self.chunk_origin_tile(chunk))
    }

    /// Converts a [`LocalTileCoord`] into a row-major flat slice index (`0..tiles_per_chunk`).
    pub const fn local_to_index(&self, local: LocalTileCoord) -> Result<usize, GridError> {
        if local.x >= self.chunk_size_tiles || local.y >= self.chunk_size_tiles {
            return Err(GridError::LocalCoordOutOfBounds {
                x: local.x,
                y: local.y,
                chunk_size: self.chunk_size_tiles,
            });
        }
        Ok((local.y * self.chunk_size_tiles + local.x) as usize)
    }

    /// Converts a flat row-major slice index back into a [`LocalTileCoord`].
    #[must_use]
    pub const fn index_to_local(&self, index: usize) -> Option<LocalTileCoord> {
        if index >= self.tiles_per_chunk() {
            return None;
        }
        let idx = index as u32;
        Some(LocalTileCoord::new(
            idx % self.chunk_size_tiles,
            idx / self.chunk_size_tiles,
        ))
    }
}
