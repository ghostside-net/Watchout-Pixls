//! Procedural World Generation for Biomes, Paths, Walls, and Resources (Phase 2).
//!
//! Uses [`WorldSeed`] and [`FastNoise2d`] to deterministically generate [`ChunkData`]
//! across an infinite 2D chunk grid. Calling [`WorldGenerator::generate_chunk`] with
//! the same seed and [`ChunkCoord`] is guaranteed to produce bit-identical [`ChunkData`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::world::coords::{ChunkCoord, LocalTileCoord, TileCoord, WorldGridConfig};
use crate::world::noise::{FastNoise2d, WorldSeed};

const SALT_ELEVATION: u64 = 0x1111_2222_3333_4444;
const SALT_MOISTURE: u64 = 0x5555_6666_7777_8888;
const SALT_PATH_PRIMARY: u64 = 0x9999_AAAA_BBBB_CCCC;
const SALT_PATH_WARP: u64 = 0xDDDD_EEEE_FFFF_1234;
const SALT_WALL_DETAIL: u64 = 0x2468_ACE0_1357_9BDF;
const SALT_RESOURCE_CLUSTER: u64 = 0xCAFE_BABE_DEAD_BEEF;
const SALT_RESOURCE_HASH: u64 = 0x0F0E_0D0C_0B0A_0908;

/// Biome classification for world tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BiomeKind {
    /// Lush pastel-mint meadow with wildflowers and herbs.
    PastelMeadow,
    /// Shallow sky-breeze lakes, rivers, and sandy banks.
    BreezeLake,
    /// Mystical lavender woodlands with timber and magic crystals.
    LavenderForest,
    /// Rocky cobblestone highlands rich in slate ore and stone walls.
    CobbleHighlands,
    /// Warm sunlit buttercup plains with berry bushes and ancient paths.
    SunlitGlade,
}

impl BiomeKind {
    /// Human-readable display name of the biome.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::PastelMeadow => "Pastel Meadow",
            Self::BreezeLake => "Breeze Lake",
            Self::LavenderForest => "Lavender Forest",
            Self::CobbleHighlands => "Cobble Highlands",
            Self::SunlitGlade => "Sunlit Glade",
        }
    }
}

/// Ground terrain surface type for a tile cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TerrainTileKind {
    /// Walkable mint meadow grass (`tile.pastel_meadow`).
    MeadowGrass,
    /// Winding packed-earth / cobblestone road (`tile.pastel_path`).
    DirtPath,
    /// Walkable shallow stream water (`tile.sky_stream`).
    ShallowWater,
    /// Impassable deep lake water (`tile.deep_water`).
    DeepWater,
    /// Soft buttercup sand bank along water edges (`tile.buttercup_sand`).
    SandBank,
    /// Soft lavender moss floor (`tile.lavender_floor`).
    LavenderMoss,
    /// Highland stone floor (`tile.cobble_floor`).
    CobbleFloor,
    /// Golden sunlit grass (`tile.sunlit_grass`).
    SunlitGrass,
}

impl TerrainTileKind {
    /// Maps this terrain kind to its corresponding `TileDefinition` ID in `GameContentRegistries`.
    #[must_use]
    pub const fn content_id(self) -> &'static str {
        match self {
            Self::MeadowGrass => "tile.pastel_meadow",
            Self::DirtPath => "tile.pastel_path",
            Self::ShallowWater => "tile.sky_stream",
            Self::DeepWater => "tile.deep_water",
            Self::SandBank => "tile.buttercup_sand",
            Self::LavenderMoss => "tile.lavender_floor",
            Self::CobbleFloor => "tile.pastel_cobble",
            Self::SunlitGrass => "tile.pastel_meadow",
        }
    }

    /// Whether the base ground terrain is walkable (ignoring walls).
    #[must_use]
    pub const fn is_base_walkable(self) -> bool {
        !matches!(self, Self::DeepWater)
    }

    /// Base movement cost of traversing this terrain tile.
    #[must_use]
    pub const fn base_movement_cost(self) -> f32 {
        match self {
            Self::DirtPath => 0.8,
            Self::MeadowGrass | Self::LavenderMoss | Self::CobbleFloor | Self::SunlitGrass => 1.0,
            Self::SandBank => 1.2,
            Self::ShallowWater => 2.0,
            Self::DeepWater => 99.0,
        }
    }
}

/// Blocking wall or structural obstacle placed on top of a tile cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WallKind {
    /// Crafted or ruin cobblestone wall.
    CobbleWall,
    /// Natural highland cliff / boulder formation.
    CliffRock,
    /// Ancient lavender-stone pillar.
    AncientPillar,
}

/// Harvestable natural resource types generated across biomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// Healing mint flora found in meadows and glades.
    MintHerb,
    /// Pastel lavender timber found in forests.
    LavenderTimber,
    /// Mineral ore vein found in rocky highlands.
    SlateOre,
    /// Arcane crystal cluster found in forests, highlands, and shores.
    CrystalCluster,
    /// Forageable berry bush found in meadows and sunlit glades.
    BerryBush,
}

impl ResourceKind {
    /// Maps this resource node to the corresponding item reward ID in `GameContentRegistries`.
    #[must_use]
    pub const fn drop_item_id(self) -> &'static str {
        match self {
            Self::MintHerb | Self::BerryBush => "item.berry_potion",
            Self::LavenderTimber | Self::CrystalCluster => "item.lavender_wand",
            Self::SlateOre => "item.pastel_sword",
        }
    }
}

/// Interactive resource node residing on a [`TileCell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceNode {
    pub kind: ResourceKind,
    pub remaining_yield: u16,
}

impl ResourceNode {
    #[must_use]
    pub const fn new(kind: ResourceKind, remaining_yield: u16) -> Self {
        Self {
            kind,
            remaining_yield,
        }
    }
}

/// Collision property of a world or instance tile (Phase 3).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum TileCollision {
    /// Walkable ground with no physical obstacle.
    #[default]
    None,
    /// Deep water hazard that blocks walking unless amphibious/swimming.
    Water,
    /// Solid wall, cliff, pillar, or out-of-bounds boundary that blocks movement.
    Solid,
}

impl TileCollision {
    /// Returns `true` if a walking player/entity can traverse this tile (`TileCollision::None`).
    #[must_use]
    pub const fn is_walkable(self) -> bool {
        matches!(self, Self::None)
    }

    /// Returns `true` if this tile blocks walking movement (`Solid` or `Water`).
    #[must_use]
    pub const fn is_blocking(self) -> bool {
        !self.is_walkable()
    }
}

/// Complete state of a single tile cell within a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileCell {
    pub biome: BiomeKind,
    pub terrain: TerrainTileKind,
    pub wall: Option<WallKind>,
    pub resource: Option<ResourceNode>,
}

impl Default for TileCell {
    fn default() -> Self {
        Self {
            biome: BiomeKind::PastelMeadow,
            terrain: TerrainTileKind::MeadowGrass,
            wall: None,
            resource: None,
        }
    }
}

impl TileCell {
    /// Evaluates the [`TileCollision`] state of this tile cell.
    #[must_use]
    pub const fn collision(&self) -> TileCollision {
        if self.wall.is_some() {
            TileCollision::Solid
        } else if matches!(self.terrain, TerrainTileKind::DeepWater) {
            TileCollision::Water
        } else if !self.terrain.is_base_walkable() {
            TileCollision::Solid
        } else {
            TileCollision::None
        }
    }

    /// Returns `true` if an entity can walk onto this tile cell (walkable ground and no blocking wall).
    #[must_use]
    pub const fn is_walkable(&self) -> bool {
        self.collision().is_walkable()
    }

    /// Returns `true` if this tile cell is part of a generated or player-built road/path.
    #[must_use]
    pub const fn is_path(&self) -> bool {
        matches!(self.terrain, TerrainTileKind::DirtPath)
    }

    /// Effective movement cost (`99.0` if blocked by a wall or deep water).
    #[must_use]
    pub const fn movement_cost(&self) -> f32 {
        if self.wall.is_some() {
            99.0
        } else {
            self.terrain.base_movement_cost()
        }
    }
}

/// Data container for a single fixed-size world chunk (`chunk_size x chunk_size` tiles).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkData {
    /// Global chunk coordinates.
    pub coord: ChunkCoord,
    /// Number of tiles along each edge of the chunk (e.g. `16`).
    pub chunk_size: u32,
    /// Row-major grid of `chunk_size * chunk_size` tile cells.
    pub tiles: Vec<TileCell>,
    /// Indicates whether any tile in this chunk has been modified after procedural generation.
    /// Unmodified chunks (`is_modified == false`) do not need their tile payload saved to disk
    /// because they can be deterministically reproduced from [`WorldSeed`].
    pub is_modified: bool,
    /// Monotonically increasing revision counter incremented on each tile modification.
    pub revision: u64,
}

impl ChunkData {
    /// Creates a new chunk filled with default meadow tiles.
    #[must_use]
    pub fn new_empty(coord: ChunkCoord, chunk_size: u32) -> Self {
        let len = (chunk_size * chunk_size) as usize;
        Self {
            coord,
            chunk_size,
            tiles: vec![TileCell::default(); len],
            is_modified: false,
            revision: 0,
        }
    }

    /// Reads the [`TileCell`] at `local` coordinates.
    #[must_use]
    pub fn get_tile(&self, local: LocalTileCoord) -> Option<&TileCell> {
        let idx = self.local_index(local)?;
        self.tiles.get(idx)
    }

    /// Mutably accesses the [`TileCell`] at `local` coordinates and marks the chunk as modified.
    pub fn get_tile_mut(&mut self, local: LocalTileCoord) -> Option<&mut TileCell> {
        let idx = self.local_index(local)?;
        let cell = self.tiles.get_mut(idx)?;
        self.is_modified = true;
        self.revision += 1;
        Some(cell)
    }

    /// Replaces the entire [`TileCell`] at `local` and marks the chunk as modified.
    pub fn set_tile(&mut self, local: LocalTileCoord, cell: TileCell) -> bool {
        let Some(idx) = self.local_index(local) else {
            return false;
        };
        if self.tiles[idx] != cell {
            self.tiles[idx] = cell;
            self.is_modified = true;
            self.revision += 1;
        }
        true
    }

    /// Places or removes a [`WallKind`] at `local` and marks the chunk as modified.
    pub fn set_wall(&mut self, local: LocalTileCoord, wall: Option<WallKind>) -> bool {
        let Some(idx) = self.local_index(local) else {
            return false;
        };
        if self.tiles[idx].wall != wall {
            self.tiles[idx].wall = wall;
            if wall.is_some() {
                // Placing a wall clears any surface resource at that exact spot
                self.tiles[idx].resource = None;
            }
            self.is_modified = true;
            self.revision += 1;
        }
        true
    }

    /// Sets the [`TerrainTileKind`] at `local` and marks the chunk as modified.
    pub fn set_terrain(&mut self, local: LocalTileCoord, terrain: TerrainTileKind) -> bool {
        let Some(idx) = self.local_index(local) else {
            return false;
        };
        if self.tiles[idx].terrain != terrain {
            self.tiles[idx].terrain = terrain;
            self.is_modified = true;
            self.revision += 1;
        }
        true
    }

    /// Harvests up to `amount` units from a [`ResourceNode`] at `local`.
    /// If the node's `remaining_yield` reaches `0`, the resource node is removed from the tile.
    /// Marks the chunk as modified whenever at least 1 unit is harvested.
    pub fn harvest_resource(
        &mut self,
        local: LocalTileCoord,
        amount: u16,
    ) -> Option<(ResourceKind, u16)> {
        if amount == 0 {
            return None;
        }
        let idx = self.local_index(local)?;
        let node = self.tiles[idx].resource?;
        let harvested = amount.min(node.remaining_yield);
        let remaining = node.remaining_yield - harvested;

        if remaining == 0 {
            self.tiles[idx].resource = None;
        } else {
            self.tiles[idx].resource = Some(ResourceNode::new(node.kind, remaining));
        }

        self.is_modified = true;
        self.revision += 1;
        Some((node.kind, harvested))
    }

    /// Counts how many tiles in this chunk belong to each [`BiomeKind`].
    #[must_use]
    pub fn biome_counts(&self) -> BTreeMap<BiomeKind, usize> {
        let mut counts = BTreeMap::new();
        for cell in &self.tiles {
            *counts.entry(cell.biome).or_insert(0) += 1;
        }
        counts
    }

    /// Returns the most common [`BiomeKind`] in this chunk.
    #[must_use]
    pub fn dominant_biome(&self) -> BiomeKind {
        self.biome_counts()
            .into_iter()
            .max_by_key(|&(_, count)| count)
            .map(|(biome, _)| biome)
            .unwrap_or(BiomeKind::PastelMeadow)
    }

    /// Number of tiles with an active [`ResourceNode`] in this chunk.
    #[must_use]
    pub fn resource_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.resource.is_some()).count()
    }

    /// Number of tiles with a blocking [`WallKind`] in this chunk.
    #[must_use]
    pub fn wall_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.wall.is_some()).count()
    }

    /// Number of path/road tiles in this chunk.
    #[must_use]
    pub fn path_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_path()).count()
    }

    #[inline]
    const fn local_index(&self, local: LocalTileCoord) -> Option<usize> {
        if local.x < self.chunk_size && local.y < self.chunk_size {
            Some((local.y * self.chunk_size + local.x) as usize)
        } else {
            None
        }
    }
}

/// Deterministic, seed-driven procedural generator for world tiles and chunks.
#[derive(Debug, Clone)]
pub struct WorldGenerator {
    seed: WorldSeed,
    elevation_noise: FastNoise2d,
    moisture_noise: FastNoise2d,
    path_noise: FastNoise2d,
    path_warp_noise: FastNoise2d,
    wall_noise: FastNoise2d,
    resource_noise: FastNoise2d,
}

impl WorldGenerator {
    /// Creates a new [`WorldGenerator`] pre-initialized with permutation tables derived from `seed`.
    #[must_use]
    pub fn new(seed: WorldSeed) -> Self {
        Self {
            seed,
            elevation_noise: FastNoise2d::new(seed.derive_layer(SALT_ELEVATION)),
            moisture_noise: FastNoise2d::new(seed.derive_layer(SALT_MOISTURE)),
            path_noise: FastNoise2d::new(seed.derive_layer(SALT_PATH_PRIMARY)),
            path_warp_noise: FastNoise2d::new(seed.derive_layer(SALT_PATH_WARP)),
            wall_noise: FastNoise2d::new(seed.derive_layer(SALT_WALL_DETAIL)),
            resource_noise: FastNoise2d::new(seed.derive_layer(SALT_RESOURCE_CLUSTER)),
        }
    }

    /// Returns the [`WorldSeed`] used by this generator.
    #[must_use]
    pub const fn seed(&self) -> WorldSeed {
        self.seed
    }

    /// Deterministically generates an entire [`ChunkData`] for `chunk_coord` and `grid_config`.
    #[must_use]
    pub fn generate_chunk(
        &self,
        chunk_coord: ChunkCoord,
        grid_config: &WorldGridConfig,
    ) -> ChunkData {
        let cs = grid_config.chunk_size_tiles;
        let mut tiles = Vec::with_capacity((cs * cs) as usize);

        for ly in 0..cs {
            for lx in 0..cs {
                let local = LocalTileCoord::new(lx, ly);
                let global_tile = grid_config.chunk_and_local_to_tile(chunk_coord, local);
                tiles.push(self.sample_tile(global_tile));
            }
        }

        ChunkData {
            coord: chunk_coord,
            chunk_size: cs,
            tiles,
            is_modified: false,
            revision: 0,
        }
    }

    /// Deterministically evaluates a single [`TileCell`] at global [`TileCoord`].
    #[must_use]
    pub fn sample_tile(&self, tile: TileCoord) -> TileCell {
        let wx = tile.x as f64;
        let wy = tile.y as f64;

        // 1. Safe starter sanctuary around Hero spawn tile (approx tile (9, 7) for world px (152, 112))
        // Guarantees the player never spawns trapped inside a rock wall or deep water.
        let spawn_center = TileCoord::new(9, 7);
        let dist_to_spawn = tile.chebyshev_distance(spawn_center);
        if dist_to_spawn <= 2 {
            return TileCell {
                biome: BiomeKind::PastelMeadow,
                terrain: if dist_to_spawn == 2
                    && (tile.x == spawn_center.x || tile.y == spawn_center.y)
                {
                    TerrainTileKind::DirtPath
                } else {
                    TerrainTileKind::MeadowGrass
                },
                wall: None,
                resource: None,
            };
        }

        // 2. Macro-scale Elevation & Moisture via multi-octave Simplex FBm
        let elevation = self
            .elevation_noise
            .fbm_simplex2d(wx, wy, 4, 0.022, 2.0, 0.5);
        let moisture = self
            .moisture_noise
            .fbm_simplex2d(wx + 137.0, wy - 91.0, 3, 0.025, 2.0, 0.5);

        // 3. Classify Biome from (elevation, moisture)
        let biome = Self::classify_biome(elevation, moisture);

        // 4. Winding Paths / Roads via domain-warped Simplex zero-crossing contour
        let warp_x = self.path_warp_noise.simplex2d(wx * 0.03, wy * 0.03) * 6.0;
        let warp_y = self
            .path_warp_noise
            .simplex2d(wx * 0.03 + 50.0, wy * 0.03 - 50.0)
            * 6.0;
        let path_contour = self
            .path_noise
            .simplex2d((wx + warp_x) * 0.028, (wy + warp_y) * 0.028)
            .abs();

        // Paths form along the zero-crossing contour everywhere above deep water
        let is_road = path_contour < 0.052 && elevation > -0.40;
        if is_road {
            return TileCell {
                biome,
                terrain: TerrainTileKind::DirtPath,
                wall: None,
                resource: None,
            };
        }

        // 5. Base Terrain & Walls per Biome
        let wall_detail = self.wall_noise.ridged_simplex2d(wx, wy, 2, 0.09, 2.0, 0.5);
        let tile_rng = self.seed.hash_unit_f64(tile.x, tile.y, SALT_RESOURCE_HASH);

        let (terrain, wall) = match biome {
            BiomeKind::BreezeLake => {
                if elevation < -0.45 {
                    (TerrainTileKind::DeepWater, None)
                } else if elevation < -0.32 {
                    (TerrainTileKind::ShallowWater, None)
                } else {
                    (TerrainTileKind::SandBank, None)
                }
            }
            BiomeKind::CobbleHighlands => {
                let wall = if elevation > 0.58 && wall_detail > 0.72 {
                    Some(WallKind::CliffRock)
                } else if wall_detail > 0.84 {
                    Some(WallKind::CobbleWall)
                } else if tile_rng < 0.015 {
                    Some(WallKind::AncientPillar)
                } else {
                    None
                };
                (TerrainTileKind::CobbleFloor, wall)
            }
            BiomeKind::LavenderForest => {
                let wall = if wall_detail > 0.89 && tile_rng < 0.25 {
                    Some(WallKind::AncientPillar)
                } else {
                    None
                };
                (TerrainTileKind::LavenderMoss, wall)
            }
            BiomeKind::SunlitGlade => {
                let wall = if wall_detail > 0.91 && tile_rng < 0.15 {
                    Some(WallKind::CobbleWall)
                } else {
                    None
                };
                (TerrainTileKind::SunlitGrass, wall)
            }
            BiomeKind::PastelMeadow => (TerrainTileKind::MeadowGrass, None),
        };

        // 6. Resource Nodes on walkable, non-water, non-wall tiles
        let resource = if wall.is_none()
            && !matches!(
                terrain,
                TerrainTileKind::DeepWater | TerrainTileKind::ShallowWater
            ) {
            self.sample_resource_node(biome, tile, tile_rng)
        } else {
            None
        };

        TileCell {
            biome,
            terrain,
            wall,
            resource,
        }
    }

    #[must_use]
    pub fn classify_biome(elevation: f64, moisture: f64) -> BiomeKind {
        if elevation < -0.25 {
            BiomeKind::BreezeLake
        } else if elevation > 0.34 {
            BiomeKind::CobbleHighlands
        } else if moisture > 0.16 {
            BiomeKind::LavenderForest
        } else if moisture < -0.18 {
            BiomeKind::SunlitGlade
        } else {
            BiomeKind::PastelMeadow
        }
    }

    fn sample_resource_node(
        &self,
        biome: BiomeKind,
        tile: TileCoord,
        tile_rng: f64,
    ) -> Option<ResourceNode> {
        let cluster = self
            .resource_noise
            .perlin2d(tile.x as f64 * 0.11, tile.y as f64 * 0.11);

        // Yield variation (deterministic 2..=6 based on secondary bit hash)
        let yield_hash = (self.seed.hash_tile(tile.x, tile.y, SALT_RESOURCE_CLUSTER) % 4) as u16;

        match biome {
            BiomeKind::PastelMeadow => {
                if tile_rng < 0.045 && cluster > -0.1 {
                    Some(ResourceNode::new(ResourceKind::MintHerb, 2 + yield_hash))
                } else if tile_rng > 0.965 {
                    Some(ResourceNode::new(ResourceKind::BerryBush, 3 + yield_hash))
                } else {
                    None
                }
            }
            BiomeKind::LavenderForest => {
                if tile_rng < 0.11 && cluster > -0.2 {
                    Some(ResourceNode::new(
                        ResourceKind::LavenderTimber,
                        4 + yield_hash,
                    ))
                } else if tile_rng > 0.975 {
                    Some(ResourceNode::new(
                        ResourceKind::CrystalCluster,
                        2 + yield_hash,
                    ))
                } else {
                    None
                }
            }
            BiomeKind::CobbleHighlands => {
                if tile_rng < 0.085 && cluster > -0.15 {
                    Some(ResourceNode::new(ResourceKind::SlateOre, 3 + yield_hash))
                } else if tile_rng > 0.972 {
                    Some(ResourceNode::new(
                        ResourceKind::CrystalCluster,
                        3 + yield_hash,
                    ))
                } else {
                    None
                }
            }
            BiomeKind::SunlitGlade => {
                if tile_rng < 0.055 {
                    Some(ResourceNode::new(ResourceKind::BerryBush, 3 + yield_hash))
                } else if tile_rng > 0.968 {
                    Some(ResourceNode::new(ResourceKind::MintHerb, 2 + yield_hash))
                } else {
                    None
                }
            }
            BiomeKind::BreezeLake => {
                // SandBank shore crystal or herb
                if tile_rng < 0.03 {
                    Some(ResourceNode::new(
                        ResourceKind::CrystalCluster,
                        2 + yield_hash,
                    ))
                } else {
                    None
                }
            }
        }
    }
}
