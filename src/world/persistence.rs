//! World Persistence & Chunk Serialization System (Phase 2).
//!
//! Efficiently serializes and persists **modified chunks** and **explored areas**
//! while omitting untouched chunks (which are deterministically reproduced from [`WorldSeed`]).
//!
//! Supported formats ([`SaveFormat`]):
//! - [`SaveFormat::Binary`]: Compact binary encoding with `"WPX2"` magic header, CRC32 integrity
//!   checksum, and Zlib compression.
//! - [`SaveFormat::CompressedRon`]: Zlib-compressed Serde RON.
//! - [`SaveFormat::CompressedJson`]: Zlib-compressed Serde JSON.
//! - [`SaveFormat::Ron`]: Human-readable Serde RON.
//! - [`SaveFormat::Json`]: Human-readable Serde JSON.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::world::coords::{ChunkCoord, WorldGridConfig};
use crate::world::generation::{
    BiomeKind, ChunkData, ResourceKind, ResourceNode, TerrainTileKind, TileCell, WallKind,
    WorldGenerator,
};
use crate::world::noise::WorldSeed;

/// Current binary and structured save schema version.
pub const WORLD_SAVE_FORMAT_VERSION: u32 = 1;

/// 4-byte magic header for [`SaveFormat::Binary`] files.
pub const BINARY_SAVE_MAGIC: &[u8; 4] = b"WPX2";

/// Errors that can occur while serializing, compressing, or loading world save data.
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("I/O error during world persistence: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid binary save header (expected 'WPX2' magic bytes)")]
    InvalidMagicHeader,
    #[error("Unsupported world save format version: {0} (expected {WORLD_SAVE_FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    #[error("CRC32 checksum mismatch in binary save (expected {expected:#010x}, computed {actual:#010x})")]
    ChecksumMismatch { expected: u32, actual: u32 },
    #[error("Corrupted or truncated binary chunk payload: {0}")]
    CorruptedBinaryPayload(String),
    #[error("RON serialization/deserialization error: {0}")]
    Ron(String),
    #[error("JSON serialization/deserialization error: {0}")]
    Json(String),
    #[error("Unsupported file extension for world save: {0}")]
    UnsupportedFormat(String),
}

/// Serialization format used for world and chunk persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SaveFormat {
    /// Compact binary encoding with `"WPX2"` header, CRC32 checksum, and Zlib compression.
    #[default]
    Binary,
    /// Zlib-compressed Rusty Object Notation (`.ron.z` / `.cron`).
    CompressedRon,
    /// Zlib-compressed JSON (`.json.z` / `.cjson`).
    CompressedJson,
    /// Uncompressed human-readable RON (`.ron`).
    Ron,
    /// Uncompressed human-readable JSON (`.json`).
    Json,
}

impl SaveFormat {
    /// Infers the [`SaveFormat`] from a file path extension.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, PersistenceError> {
        let name = path
            .as_ref()
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();

        if name.ends_with(".bin") || name.ends_with(".wpx") || name.ends_with(".bincode") {
            Ok(Self::Binary)
        } else if name.ends_with(".ron.z") || name.ends_with(".ron.gz") || name.ends_with(".cron") {
            Ok(Self::CompressedRon)
        } else if name.ends_with(".json.z")
            || name.ends_with(".json.gz")
            || name.ends_with(".cjson")
        {
            Ok(Self::CompressedJson)
        } else if name.ends_with(".ron") {
            Ok(Self::Ron)
        } else if name.ends_with(".json") {
            Ok(Self::Json)
        } else {
            Err(PersistenceError::UnsupportedFormat(name))
        }
    }
}

/// Indicates whether a loaded chunk came from persisted player modifications or fresh seed generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChunkSource {
    /// Deterministically generated from [`WorldSeed`] (chunk had no saved modifications).
    GeneratedFromSeed,
    /// Restored from [`WorldPersistence`] because the chunk was previously modified.
    RestoredModified,
}

/// Lightweight metadata record for an explored chunk (fog-of-war / world map discovery).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExploredChunkRecord {
    pub coord: ChunkCoord,
    pub dominant_biome: BiomeKind,
    pub first_discovered_tick: u64,
    pub visit_count: u32,
}

/// Single-tile difference against the procedural seed baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileDelta {
    pub local_index: u16,
    pub cell: TileCell,
}

/// Complete serializable snapshot of a world's seed, explored regions, and modified chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSaveData {
    pub format_version: u32,
    pub seed: WorldSeed,
    pub grid_config: WorldGridConfig,
    pub explored_chunks: Vec<ExploredChunkRecord>,
    pub modified_chunks: Vec<ChunkData>,
}

impl WorldSaveData {
    /// Computes the exact list of [`TileDelta`]s for a modified chunk compared to its
    /// deterministic seed baseline.
    #[must_use]
    pub fn compute_chunk_deltas(&self, modified_chunk: &ChunkData) -> Vec<TileDelta> {
        let generator = WorldGenerator::new(self.seed);
        let baseline = generator.generate_chunk(modified_chunk.coord, &self.grid_config);
        let mut deltas = Vec::new();
        for (idx, (&actual, &base)) in modified_chunk
            .tiles
            .iter()
            .zip(baseline.tiles.iter())
            .enumerate()
        {
            if actual != base {
                deltas.push(TileDelta {
                    local_index: idx as u16,
                    cell: actual,
                });
            }
        }
        deltas
    }

    /// Reconstructs a modified [`ChunkData`] by generating the baseline chunk from `seed`
    /// and applying a slice of [`TileDelta`]s on top.
    #[must_use]
    pub fn reconstruct_chunk_from_deltas(
        seed: WorldSeed,
        grid_config: &WorldGridConfig,
        coord: ChunkCoord,
        deltas: &[TileDelta],
        revision: u64,
    ) -> ChunkData {
        let generator = WorldGenerator::new(seed);
        let mut chunk = generator.generate_chunk(coord, grid_config);
        for delta in deltas {
            if let Some(local) = grid_config.index_to_local(delta.local_index as usize) {
                let _ = chunk.set_tile(local, delta.cell);
            }
        }
        chunk.is_modified = !deltas.is_empty();
        chunk.revision = revision;
        chunk
    }
}

/// Bevy Resource managing in-memory and on-disk persistence of explored areas and modified chunks.
#[derive(Resource, Debug, Default, Clone)]
pub struct WorldPersistence {
    explored_chunks: BTreeMap<ChunkCoord, ExploredChunkRecord>,
    modified_chunks: BTreeMap<ChunkCoord, ChunkData>,
    pub save_file_path: Option<PathBuf>,
}

impl WorldPersistence {
    /// Creates an empty [`WorldPersistence`] store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a [`WorldPersistence`] configured with a target save file path.
    #[must_use]
    pub fn with_save_path(path: impl Into<PathBuf>) -> Self {
        Self {
            save_file_path: Some(path.into()),
            ..Self::default()
        }
    }

    /// Marks `chunk.coord` as explored at `current_tick`, or increments its `visit_count`
    /// if it was already explored previously.
    pub fn mark_chunk_explored(&mut self, chunk: &ChunkData, current_tick: u64) {
        let dominant_biome = chunk.dominant_biome();
        self.explored_chunks
            .entry(chunk.coord)
            .and_modify(|rec| {
                rec.visit_count = rec.visit_count.saturating_add(1);
                rec.dominant_biome = dominant_biome;
            })
            .or_insert(ExploredChunkRecord {
                coord: chunk.coord,
                dominant_biome,
                first_discovered_tick: current_tick,
                visit_count: 1,
            });
    }

    /// Returns `true` if `coord` has been explored by the player.
    #[must_use]
    pub fn is_chunk_explored(&self, coord: ChunkCoord) -> bool {
        self.explored_chunks.contains_key(&coord)
    }

    /// Retrieves the [`ExploredChunkRecord`] for `coord`, if explored.
    #[must_use]
    pub fn get_explored_record(&self, coord: ChunkCoord) -> Option<&ExploredChunkRecord> {
        self.explored_chunks.get(&coord)
    }

    /// Total number of explored chunks recorded.
    #[must_use]
    pub fn explored_chunk_count(&self) -> usize {
        self.explored_chunks.len()
    }

    /// Iterates over all explored chunk records in deterministic coordinate order.
    pub fn explored_chunks(&self) -> impl Iterator<Item = (&ChunkCoord, &ExploredChunkRecord)> {
        self.explored_chunks.iter()
    }

    /// Stores `chunk` into the persistence map **only if** `chunk.is_modified` is `true`.
    /// Untouched chunks (`!chunk.is_modified`) are intentionally skipped so they can be
    /// deterministically reproduced from [`WorldSeed`].
    pub fn store_chunk_if_modified(&mut self, chunk: &ChunkData) -> bool {
        if chunk.is_modified {
            self.modified_chunks.insert(chunk.coord, chunk.clone());
            true
        } else {
            false
        }
    }

    /// Returns `true` if a modified version of `coord` is stored.
    #[must_use]
    pub fn has_modified_chunk(&self, coord: ChunkCoord) -> bool {
        self.modified_chunks.contains_key(&coord)
    }

    /// Retrieves a stored modified [`ChunkData`] for `coord`, if present.
    #[must_use]
    pub fn get_modified_chunk(&self, coord: ChunkCoord) -> Option<&ChunkData> {
        self.modified_chunks.get(&coord)
    }

    /// Total number of modified chunks currently persisted.
    #[must_use]
    pub fn modified_chunk_count(&self) -> usize {
        self.modified_chunks.len()
    }

    /// Resolves a chunk for `coord`:
    /// - If `coord` was previously modified, returns the saved [`ChunkData`] (`ChunkSource::RestoredModified`).
    /// - Otherwise, deterministically generates the chunk from `seed` (`ChunkSource::GeneratedFromSeed`).
    #[must_use]
    pub fn load_or_generate_chunk(
        &self,
        coord: ChunkCoord,
        seed: WorldSeed,
        grid_config: &WorldGridConfig,
    ) -> (ChunkData, ChunkSource) {
        if let Some(saved) = self.modified_chunks.get(&coord) {
            (saved.clone(), ChunkSource::RestoredModified)
        } else {
            let generator = WorldGenerator::new(seed);
            (
                generator.generate_chunk(coord, grid_config),
                ChunkSource::GeneratedFromSeed,
            )
        }
    }

    /// Builds a complete [`WorldSaveData`] snapshot combining stored state and any currently active
    /// modified chunks in `active_chunks`.
    #[must_use]
    pub fn build_save_data(
        &self,
        seed: WorldSeed,
        grid_config: WorldGridConfig,
        active_chunks: Option<&BTreeMap<ChunkCoord, ChunkData>>,
    ) -> WorldSaveData {
        let mut merged_modified = self.modified_chunks.clone();
        if let Some(active) = active_chunks {
            for (coord, chunk) in active {
                if chunk.is_modified {
                    merged_modified.insert(*coord, chunk.clone());
                }
            }
        }

        WorldSaveData {
            format_version: WORLD_SAVE_FORMAT_VERSION,
            seed,
            grid_config,
            explored_chunks: self.explored_chunks.values().copied().collect(),
            modified_chunks: merged_modified.into_values().collect(),
        }
    }

    /// Restores [`WorldPersistence`] state from a deserialized [`WorldSaveData`] snapshot.
    pub fn restore_from_save_data(&mut self, data: WorldSaveData) {
        self.explored_chunks.clear();
        for rec in data.explored_chunks {
            self.explored_chunks.insert(rec.coord, rec);
        }

        self.modified_chunks.clear();
        for mut chunk in data.modified_chunks {
            chunk.is_modified = true;
            self.modified_chunks.insert(chunk.coord, chunk);
        }
    }

    /// Serializes the world persistence state into a byte buffer using the specified [`SaveFormat`].
    pub fn serialize_bytes(
        &self,
        seed: WorldSeed,
        grid_config: WorldGridConfig,
        active_chunks: Option<&BTreeMap<ChunkCoord, ChunkData>>,
        format: SaveFormat,
    ) -> Result<Vec<u8>, PersistenceError> {
        let save_data = self.build_save_data(seed, grid_config, active_chunks);
        encode_save_data(&save_data, format)
    }

    /// Deserializes a [`WorldSaveData`] snapshot from a byte slice using the specified [`SaveFormat`].
    pub fn deserialize_bytes(
        bytes: &[u8],
        format: SaveFormat,
    ) -> Result<WorldSaveData, PersistenceError> {
        decode_save_data(bytes, format)
    }

    /// Saves the world persistence state to `path` using the given [`SaveFormat`].
    /// Returns the number of bytes written to disk.
    pub fn save_to_file(
        &self,
        path: impl AsRef<Path>,
        seed: WorldSeed,
        grid_config: WorldGridConfig,
        active_chunks: Option<&BTreeMap<ChunkCoord, ChunkData>>,
        format: SaveFormat,
    ) -> Result<usize, PersistenceError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let bytes = self.serialize_bytes(seed, grid_config, active_chunks, format)?;
        fs::write(path, &bytes)?;
        Ok(bytes.len())
    }

    /// Reads and deserializes [`WorldSaveData`] from `path` using the given [`SaveFormat`].
    pub fn load_from_file(
        path: impl AsRef<Path>,
        format: SaveFormat,
    ) -> Result<WorldSaveData, PersistenceError> {
        let bytes = fs::read(path)?;
        Self::deserialize_bytes(&bytes, format)
    }
}

// ============================================================================
// Encoding / Decoding Implementation (Binary + Compressed RON/JSON + Plain RON/JSON)
// ============================================================================

fn compress_zlib(raw: &[u8]) -> Result<Vec<u8>, PersistenceError> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(raw)?;
    Ok(encoder.finish()?)
}

fn decompress_zlib(compressed: &[u8]) -> Result<Vec<u8>, PersistenceError> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

fn encode_save_data(data: &WorldSaveData, format: SaveFormat) -> Result<Vec<u8>, PersistenceError> {
    match format {
        SaveFormat::Binary => encode_binary_save(data),
        SaveFormat::Ron => {
            #[cfg(feature = "ron-content")]
            {
                let s = ron::to_string(data).map_err(|e| PersistenceError::Ron(e.to_string()))?;
                Ok(s.into_bytes())
            }
            #[cfg(not(feature = "ron-content"))]
            {
                Err(PersistenceError::UnsupportedFormat("ron".to_string()))
            }
        }
        SaveFormat::CompressedRon => {
            #[cfg(feature = "ron-content")]
            {
                let s = ron::to_string(data).map_err(|e| PersistenceError::Ron(e.to_string()))?;
                compress_zlib(s.as_bytes())
            }
            #[cfg(not(feature = "ron-content"))]
            {
                Err(PersistenceError::UnsupportedFormat(
                    "compressed-ron".to_string(),
                ))
            }
        }
        SaveFormat::Json => {
            #[cfg(feature = "json-content")]
            {
                serde_json::to_vec(data).map_err(|e| PersistenceError::Json(e.to_string()))
            }
            #[cfg(not(feature = "json-content"))]
            {
                Err(PersistenceError::UnsupportedFormat("json".to_string()))
            }
        }
        SaveFormat::CompressedJson => {
            #[cfg(feature = "json-content")]
            {
                let raw =
                    serde_json::to_vec(data).map_err(|e| PersistenceError::Json(e.to_string()))?;
                compress_zlib(&raw)
            }
            #[cfg(not(feature = "json-content"))]
            {
                Err(PersistenceError::UnsupportedFormat(
                    "compressed-json".to_string(),
                ))
            }
        }
    }
}

fn decode_save_data(bytes: &[u8], format: SaveFormat) -> Result<WorldSaveData, PersistenceError> {
    match format {
        SaveFormat::Binary => decode_binary_save(bytes),
        SaveFormat::Ron => {
            #[cfg(feature = "ron-content")]
            {
                let s =
                    std::str::from_utf8(bytes).map_err(|e| PersistenceError::Ron(e.to_string()))?;
                ron::from_str(s).map_err(|e| PersistenceError::Ron(e.to_string()))
            }
            #[cfg(not(feature = "ron-content"))]
            {
                let _ = bytes;
                Err(PersistenceError::UnsupportedFormat("ron".to_string()))
            }
        }
        SaveFormat::CompressedRon => {
            #[cfg(feature = "ron-content")]
            {
                let decompressed = decompress_zlib(bytes)?;
                let s = std::str::from_utf8(&decompressed)
                    .map_err(|e| PersistenceError::Ron(e.to_string()))?;
                ron::from_str(s).map_err(|e| PersistenceError::Ron(e.to_string()))
            }
            #[cfg(not(feature = "ron-content"))]
            {
                let _ = bytes;
                Err(PersistenceError::UnsupportedFormat(
                    "compressed-ron".to_string(),
                ))
            }
        }
        SaveFormat::Json => {
            #[cfg(feature = "json-content")]
            {
                serde_json::from_slice(bytes).map_err(|e| PersistenceError::Json(e.to_string()))
            }
            #[cfg(not(feature = "json-content"))]
            {
                let _ = bytes;
                Err(PersistenceError::UnsupportedFormat("json".to_string()))
            }
        }
        SaveFormat::CompressedJson => {
            #[cfg(feature = "json-content")]
            {
                let decompressed = decompress_zlib(bytes)?;
                serde_json::from_slice(&decompressed)
                    .map_err(|e| PersistenceError::Json(e.to_string()))
            }
            #[cfg(not(feature = "json-content"))]
            {
                let _ = bytes;
                Err(PersistenceError::UnsupportedFormat(
                    "compressed-json".to_string(),
                ))
            }
        }
    }
}

// ============================================================================
// Deterministic Binary Codec ("WPX2" + Version + CRC32 + Zlib-compressed Bincode-style payload)
// ============================================================================

fn encode_binary_save(data: &WorldSaveData) -> Result<Vec<u8>, PersistenceError> {
    let mut payload = Vec::with_capacity(512 + data.modified_chunks.len() * 1024);

    // 1. World seed & grid config
    payload.extend_from_slice(&data.seed.0.to_le_bytes());
    payload.extend_from_slice(&data.grid_config.tile_size_px.to_le_bytes());
    payload.extend_from_slice(&data.grid_config.chunk_size_tiles.to_le_bytes());

    // 2. Explored chunks
    payload.extend_from_slice(&(data.explored_chunks.len() as u32).to_le_bytes());
    for rec in &data.explored_chunks {
        payload.extend_from_slice(&rec.coord.x.to_le_bytes());
        payload.extend_from_slice(&rec.coord.y.to_le_bytes());
        payload.push(biome_to_u8(rec.dominant_biome));
        payload.extend_from_slice(&rec.first_discovered_tick.to_le_bytes());
        payload.extend_from_slice(&rec.visit_count.to_le_bytes());
    }

    // 3. Modified chunks
    payload.extend_from_slice(&(data.modified_chunks.len() as u32).to_le_bytes());
    for chunk in &data.modified_chunks {
        payload.extend_from_slice(&chunk.coord.x.to_le_bytes());
        payload.extend_from_slice(&chunk.coord.y.to_le_bytes());
        payload.extend_from_slice(&chunk.chunk_size.to_le_bytes());
        payload.push(u8::from(chunk.is_modified));
        payload.extend_from_slice(&chunk.revision.to_le_bytes());
        payload.extend_from_slice(&(chunk.tiles.len() as u32).to_le_bytes());

        for cell in &chunk.tiles {
            payload.push(biome_to_u8(cell.biome));
            payload.push(terrain_to_u8(cell.terrain));
            payload.push(wall_to_u8(cell.wall));
            match cell.resource {
                None => payload.push(0),
                Some(node) => {
                    payload.push(resource_to_u8(node.kind));
                    payload.extend_from_slice(&node.remaining_yield.to_le_bytes());
                }
            }
        }
    }

    let crc = crc32fast::hash(&payload);
    let compressed = compress_zlib(&payload)?;

    let mut out = Vec::with_capacity(16 + compressed.len());
    out.extend_from_slice(BINARY_SAVE_MAGIC);
    out.extend_from_slice(&data.format_version.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn decode_binary_save(bytes: &[u8]) -> Result<WorldSaveData, PersistenceError> {
    if bytes.len() < 16 {
        return Err(PersistenceError::InvalidMagicHeader);
    }
    if &bytes[0..4] != BINARY_SAVE_MAGIC {
        return Err(PersistenceError::InvalidMagicHeader);
    }

    let format_version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if format_version != WORLD_SAVE_FORMAT_VERSION {
        return Err(PersistenceError::UnsupportedVersion(format_version));
    }

    let expected_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let expected_crc = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);

    let payload = decompress_zlib(&bytes[16..])?;
    if payload.len() != expected_len {
        return Err(PersistenceError::CorruptedBinaryPayload(format!(
            "decompressed length {} does not match header {}",
            payload.len(),
            expected_len
        )));
    }

    let actual_crc = crc32fast::hash(&payload);
    if actual_crc != expected_crc {
        return Err(PersistenceError::ChecksumMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }

    let mut cursor = BinaryCursor::new(&payload);
    let seed = WorldSeed(cursor.read_u64()?);
    let tile_size_px = cursor.read_u32()?;
    let chunk_size_tiles = cursor.read_u32()?;
    let grid_config = WorldGridConfig::new(tile_size_px, chunk_size_tiles)
        .map_err(|e| PersistenceError::CorruptedBinaryPayload(e.to_string()))?;

    let explored_count = cursor.read_u32()? as usize;
    let mut explored_chunks = Vec::with_capacity(explored_count);
    for _ in 0..explored_count {
        let cx = cursor.read_i32()?;
        let cy = cursor.read_i32()?;
        let dominant_biome = u8_to_biome(cursor.read_u8()?)?;
        let first_discovered_tick = cursor.read_u64()?;
        let visit_count = cursor.read_u32()?;
        explored_chunks.push(ExploredChunkRecord {
            coord: ChunkCoord::new(cx, cy),
            dominant_biome,
            first_discovered_tick,
            visit_count,
        });
    }

    let modified_count = cursor.read_u32()? as usize;
    let mut modified_chunks = Vec::with_capacity(modified_count);
    for _ in 0..modified_count {
        let cx = cursor.read_i32()?;
        let cy = cursor.read_i32()?;
        let chunk_size = cursor.read_u32()?;
        let is_modified = cursor.read_u8()? != 0;
        let revision = cursor.read_u64()?;
        let tile_count = cursor.read_u32()? as usize;

        if tile_count != (chunk_size * chunk_size) as usize {
            return Err(PersistenceError::CorruptedBinaryPayload(format!(
                "tile count {tile_count} does not match chunk_size {chunk_size}x{chunk_size}"
            )));
        }

        let mut tiles = Vec::with_capacity(tile_count);
        for _ in 0..tile_count {
            let biome = u8_to_biome(cursor.read_u8()?)?;
            let terrain = u8_to_terrain(cursor.read_u8()?)?;
            let wall = u8_to_wall(cursor.read_u8()?)?;
            let res_tag = cursor.read_u8()?;
            let resource = if res_tag == 0 {
                None
            } else {
                let kind = u8_to_resource(res_tag)?;
                let remaining_yield = cursor.read_u16()?;
                Some(ResourceNode::new(kind, remaining_yield))
            };
            tiles.push(TileCell {
                biome,
                terrain,
                wall,
                resource,
            });
        }

        modified_chunks.push(ChunkData {
            coord: ChunkCoord::new(cx, cy),
            chunk_size,
            tiles,
            is_modified,
            revision,
        });
    }

    Ok(WorldSaveData {
        format_version,
        seed,
        grid_config,
        explored_chunks,
        modified_chunks,
    })
}

struct BinaryCursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BinaryCursor<'a> {
    const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], PersistenceError> {
        if self.pos + N > self.buf.len() {
            return Err(PersistenceError::CorruptedBinaryPayload(
                "unexpected end of binary stream".to_string(),
            ));
        }
        let mut arr = [0u8; N];
        arr.copy_from_slice(&self.buf[self.pos..self.pos + N]);
        self.pos += N;
        Ok(arr)
    }

    fn read_u8(&mut self) -> Result<u8, PersistenceError> {
        Ok(self.read_exact::<1>()?[0])
    }

    fn read_u16(&mut self) -> Result<u16, PersistenceError> {
        Ok(u16::from_le_bytes(self.read_exact::<2>()?))
    }

    fn read_u32(&mut self) -> Result<u32, PersistenceError> {
        Ok(u32::from_le_bytes(self.read_exact::<4>()?))
    }

    fn read_i32(&mut self) -> Result<i32, PersistenceError> {
        Ok(i32::from_le_bytes(self.read_exact::<4>()?))
    }

    fn read_u64(&mut self) -> Result<u64, PersistenceError> {
        Ok(u64::from_le_bytes(self.read_exact::<8>()?))
    }
}

const fn biome_to_u8(b: BiomeKind) -> u8 {
    match b {
        BiomeKind::PastelMeadow => 0,
        BiomeKind::BreezeLake => 1,
        BiomeKind::LavenderForest => 2,
        BiomeKind::CobbleHighlands => 3,
        BiomeKind::SunlitGlade => 4,
    }
}

fn u8_to_biome(v: u8) -> Result<BiomeKind, PersistenceError> {
    match v {
        0 => Ok(BiomeKind::PastelMeadow),
        1 => Ok(BiomeKind::BreezeLake),
        2 => Ok(BiomeKind::LavenderForest),
        3 => Ok(BiomeKind::CobbleHighlands),
        4 => Ok(BiomeKind::SunlitGlade),
        other => Err(PersistenceError::CorruptedBinaryPayload(format!(
            "invalid BiomeKind tag {other}"
        ))),
    }
}

const fn terrain_to_u8(t: TerrainTileKind) -> u8 {
    match t {
        TerrainTileKind::MeadowGrass => 0,
        TerrainTileKind::DirtPath => 1,
        TerrainTileKind::ShallowWater => 2,
        TerrainTileKind::DeepWater => 3,
        TerrainTileKind::SandBank => 4,
        TerrainTileKind::LavenderMoss => 5,
        TerrainTileKind::CobbleFloor => 6,
        TerrainTileKind::SunlitGrass => 7,
    }
}

fn u8_to_terrain(v: u8) -> Result<TerrainTileKind, PersistenceError> {
    match v {
        0 => Ok(TerrainTileKind::MeadowGrass),
        1 => Ok(TerrainTileKind::DirtPath),
        2 => Ok(TerrainTileKind::ShallowWater),
        3 => Ok(TerrainTileKind::DeepWater),
        4 => Ok(TerrainTileKind::SandBank),
        5 => Ok(TerrainTileKind::LavenderMoss),
        6 => Ok(TerrainTileKind::CobbleFloor),
        7 => Ok(TerrainTileKind::SunlitGrass),
        other => Err(PersistenceError::CorruptedBinaryPayload(format!(
            "invalid TerrainTileKind tag {other}"
        ))),
    }
}

const fn wall_to_u8(w: Option<WallKind>) -> u8 {
    match w {
        None => 0,
        Some(WallKind::CobbleWall) => 1,
        Some(WallKind::CliffRock) => 2,
        Some(WallKind::AncientPillar) => 3,
    }
}

fn u8_to_wall(v: u8) -> Result<Option<WallKind>, PersistenceError> {
    match v {
        0 => Ok(None),
        1 => Ok(Some(WallKind::CobbleWall)),
        2 => Ok(Some(WallKind::CliffRock)),
        3 => Ok(Some(WallKind::AncientPillar)),
        other => Err(PersistenceError::CorruptedBinaryPayload(format!(
            "invalid WallKind tag {other}"
        ))),
    }
}

const fn resource_to_u8(r: ResourceKind) -> u8 {
    match r {
        ResourceKind::MintHerb => 1,
        ResourceKind::LavenderTimber => 2,
        ResourceKind::SlateOre => 3,
        ResourceKind::CrystalCluster => 4,
        ResourceKind::BerryBush => 5,
    }
}

fn u8_to_resource(v: u8) -> Result<ResourceKind, PersistenceError> {
    match v {
        1 => Ok(ResourceKind::MintHerb),
        2 => Ok(ResourceKind::LavenderTimber),
        3 => Ok(ResourceKind::SlateOre),
        4 => Ok(ResourceKind::CrystalCluster),
        5 => Ok(ResourceKind::BerryBush),
        other => Err(PersistenceError::CorruptedBinaryPayload(format!(
            "invalid ResourceKind tag {other}"
        ))),
    }
}
