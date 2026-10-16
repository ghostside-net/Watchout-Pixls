//! Data-Driven Content Definitions & Registries (Items, Classes, Entities, Tiles).
//!
//! Defines the core `ContentDefinition` trait and strongly-typed registries
//! capable of loading external definitions from RON (`.ron`) and JSON (`.json`) files
//! or runtime mod plugins.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::fs;
use std::path::Path;

use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur when validating or loading content definitions.
#[derive(Debug, Error)]
pub enum ContentError {
    #[error("Content ID must not be empty for {content_kind}")]
    EmptyId { content_kind: &'static str },
    #[error("Display name must not be empty for {content_kind} '{id}'")]
    EmptyDisplayName {
        content_kind: &'static str,
        id: String,
    },
    #[error("Invalid attribute or property in {content_kind} '{id}': {reason}")]
    ValidationFailed {
        content_kind: &'static str,
        id: String,
        reason: String,
    },
    #[error("Duplicate content ID '{id}' in registry '{content_kind}'")]
    DuplicateId {
        content_kind: &'static str,
        id: String,
    },
    #[error("Unsupported or disabled data format: {0}")]
    UnsupportedFormat(String),
    #[error("I/O error while reading content file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse RON content: {0}")]
    RonParse(String),
    #[error("Failed to parse JSON content: {0}")]
    JsonParse(String),
}

/// Supported external serialization formats for content packs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFormat {
    /// Rusty Object Notation (`.ron`).
    Ron,
    /// JavaScript Object Notation (`.json`).
    Json,
}

impl DataFormat {
    /// Detects the `DataFormat` from a file path extension.
    pub fn from_path(path: &Path) -> Result<Self, ContentError> {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("ron") => Ok(Self::Ron),
            Some("json") => Ok(Self::Json),
            other => Err(ContentError::UnsupportedFormat(
                other.unwrap_or("<none>").to_string(),
            )),
        }
    }
}

/// Core trait implemented by all modular game content definitions
/// (Items, Classes, Entities, Tiles, etc.).
pub trait ContentDefinition:
    Send + Sync + 'static + Clone + Debug + Serialize + DeserializeOwned
{
    /// Human-readable category identifier (e.g. `"Item"`, `"Class"`, `"Entity"`, `"Tile"`).
    const CONTENT_KIND: &'static str;

    /// Unique registry identifier (e.g. `"item.pastel_blade"`).
    fn id(&self) -> &str;

    /// Localized or human-readable display name.
    fn display_name(&self) -> &str;

    /// Validates domain-specific invariants after deserialization.
    fn validate(&self) -> Result<(), ContentError> {
        if self.id().trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name().trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id().to_string(),
            });
        }
        Ok(())
    }
}

/// Generic, deterministic registry for any `ContentDefinition` type `T`.
#[derive(Debug, Clone)]
pub struct ContentRegistry<T: ContentDefinition> {
    entries: BTreeMap<String, T>,
}

impl<T: ContentDefinition> Default for ContentRegistry<T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }
}

impl<T: ContentDefinition> ContentRegistry<T> {
    /// Creates an empty `ContentRegistry`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates and registers a single definition, failing if the ID is already registered.
    pub fn register(&mut self, definition: T) -> Result<(), ContentError> {
        definition.validate()?;
        let id = definition.id().to_string();
        if self.entries.contains_key(&id) {
            return Err(ContentError::DuplicateId {
                content_kind: T::CONTENT_KIND,
                id,
            });
        }
        self.entries.insert(id, definition);
        Ok(())
    }

    /// Validates and registers or overwrites an existing definition (useful for mod overrides).
    pub fn register_or_override(&mut self, definition: T) -> Result<Option<T>, ContentError> {
        definition.validate()?;
        Ok(self.entries.insert(definition.id().to_string(), definition))
    }

    /// Retrieves a definition by its unique ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&T> {
        self.entries.get(id)
    }

    /// Returns the number of registered definitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if no definitions are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over all `(id, definition)` pairs in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Deserializes and registers a single definition from a RON or JSON string.
    pub fn load_single_from_str(
        &mut self,
        raw: &str,
        format: DataFormat,
    ) -> Result<String, ContentError> {
        let def: T = Self::deserialize_str(raw, format)?;
        let id = def.id().to_string();
        self.register_or_override(def)?;
        Ok(id)
    }

    /// Deserializes and registers a list of definitions (`Vec<T>`) from a RON or JSON string.
    pub fn load_list_from_str(
        &mut self,
        raw: &str,
        format: DataFormat,
    ) -> Result<usize, ContentError> {
        let defs: Vec<T> = Self::deserialize_str(raw, format)?;
        let count = defs.len();
        for def in defs {
            self.register_or_override(def)?;
        }
        Ok(count)
    }

    /// Loads a list of definitions (`Vec<T>`) from a `.ron` or `.json` file on disk.
    pub fn load_list_from_file(&mut self, path: impl AsRef<Path>) -> Result<usize, ContentError> {
        let path = path.as_ref();
        let format = DataFormat::from_path(path)?;
        let raw = fs::read_to_string(path)?;
        self.load_list_from_str(&raw, format)
    }

    fn deserialize_str<D: DeserializeOwned>(
        raw: &str,
        format: DataFormat,
    ) -> Result<D, ContentError> {
        match format {
            DataFormat::Ron => {
                #[cfg(feature = "ron-content")]
                {
                    ron::from_str(raw).map_err(|e| ContentError::RonParse(e.to_string()))
                }
                #[cfg(not(feature = "ron-content"))]
                {
                    let _ = raw;
                    Err(ContentError::UnsupportedFormat("ron".to_string()))
                }
            }
            DataFormat::Json => {
                #[cfg(feature = "json-content")]
                {
                    serde_json::from_str(raw).map_err(|e| ContentError::JsonParse(e.to_string()))
                }
                #[cfg(not(feature = "json-content"))]
                {
                    let _ = raw;
                    Err(ContentError::UnsupportedFormat("json".to_string()))
                }
            }
        }
    }
}

// ============================================================================
// 1. Item Content Definition
// ============================================================================

/// Category of an in-game item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemCategory {
    Weapon,
    Armor,
    Consumable,
    Tool,
    Material,
    KeyItem,
}

/// Data-driven definition for an Item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemDefinition {
    pub id: String,
    pub display_name: String,
    pub category: ItemCategory,
    pub max_stack: u32,
    pub value: u32,
    pub power_bonus: i32,
    pub sprite_id: String,
}

impl ContentDefinition for ItemDefinition {
    const CONTENT_KIND: &'static str = "Item";

    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.id.trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
            });
        }
        if self.max_stack == 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "max_stack must be >= 1".to_string(),
            });
        }
        Ok(())
    }
}

// ============================================================================
// 2. Class Content Definition
// ============================================================================

/// Data-driven definition for a playable or NPC Character Class.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassDefinition {
    pub id: String,
    pub display_name: String,
    pub base_health: i32,
    pub base_mana: i32,
    pub base_speed: f32,
    pub starting_items: Vec<String>,
    pub passive_trait: String,
}

impl ContentDefinition for ClassDefinition {
    const CONTENT_KIND: &'static str = "Class";

    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.id.trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
            });
        }
        if self.base_health <= 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "base_health must be > 0".to_string(),
            });
        }
        if self.base_speed <= 0.0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "base_speed must be > 0.0".to_string(),
            });
        }
        Ok(())
    }
}

// ============================================================================
// 3. Entity Content Definition
// ============================================================================

/// Archetype classification for world entities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityArchetype {
    Player,
    Npc,
    Monster,
    Critter,
    InteractableProp,
}

/// Data-driven definition for a spawnable World Entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDefinition {
    pub id: String,
    pub display_name: String,
    pub archetype: EntityArchetype,
    pub max_health: i32,
    pub move_speed: f32,
    pub hostile: bool,
    pub sprite_id: String,
    pub drop_table: Vec<String>,
}

impl ContentDefinition for EntityDefinition {
    const CONTENT_KIND: &'static str = "Entity";

    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.id.trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
            });
        }
        if self.max_health <= 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "max_health must be > 0".to_string(),
            });
        }
        Ok(())
    }
}

// ============================================================================
// 4. Tile Content Definition
// ============================================================================

/// Data-driven definition for a 2D World Tile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileDefinition {
    pub id: String,
    pub display_name: String,
    pub walkable: bool,
    pub transparent: bool,
    pub movement_cost: f32,
    pub sprite_id: String,
    /// Optional 8x8 palette index matrix (64 entries) for custom modded tile graphics.
    pub custom_indices_8x8: Option<Vec<u8>>,
}

impl ContentDefinition for TileDefinition {
    const CONTENT_KIND: &'static str = "Tile";

    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn validate(&self) -> Result<(), ContentError> {
        if self.id.trim().is_empty() {
            return Err(ContentError::EmptyId {
                content_kind: Self::CONTENT_KIND,
            });
        }
        if self.display_name.trim().is_empty() {
            return Err(ContentError::EmptyDisplayName {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
            });
        }
        if self.movement_cost <= 0.0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "movement_cost must be > 0.0".to_string(),
            });
        }
        if let Some(ref indices) = self.custom_indices_8x8 {
            if indices.len() != 64 {
                return Err(ContentError::ValidationFailed {
                    content_kind: Self::CONTENT_KIND,
                    id: self.id.clone(),
                    reason: format!(
                        "custom_indices_8x8 must have exactly 64 entries, got {}",
                        indices.len()
                    ),
                });
            }
        }
        Ok(())
    }
}

/// Central Bevy Resource holding all typed content registries.
#[derive(Resource, Debug, Default, Clone)]
pub struct GameContentRegistries {
    pub items: ContentRegistry<ItemDefinition>,
    pub classes: ContentRegistry<ClassDefinition>,
    pub entities: ContentRegistry<EntityDefinition>,
    pub tiles: ContentRegistry<TileDefinition>,
}

impl GameContentRegistries {
    /// Creates a new empty set of registries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Populates the registries with built-in core content definitions so the
    /// base game always has valid fallback items, classes, entities, and tiles.
    pub fn register_core_defaults(&mut self) -> Result<(), ContentError> {
        self.items.register_or_override(ItemDefinition {
            id: "item.pastel_sword".to_string(),
            display_name: "Pastel Mint Blade".to_string(),
            category: ItemCategory::Weapon,
            max_stack: 1,
            value: 120,
            power_bonus: 8,
            sprite_id: "sprite_pastel_sword".to_string(),
        })?;
        self.items.register_or_override(ItemDefinition {
            id: "item.berry_potion".to_string(),
            display_name: "Slate-Berry Tonic".to_string(),
            category: ItemCategory::Consumable,
            max_stack: 16,
            value: 25,
            power_bonus: 15,
            sprite_id: "sprite_berry_potion".to_string(),
        })?;

        self.classes.register_or_override(ClassDefinition {
            id: "class.pixel_ranger".to_string(),
            display_name: "Pixel Ranger".to_string(),
            base_health: 100,
            base_mana: 40,
            base_speed: 64.0,
            starting_items: vec![
                "item.pastel_sword".to_string(),
                "item.berry_potion".to_string(),
            ],
            passive_trait: "Snap Step".to_string(),
        })?;

        self.entities.register_or_override(EntityDefinition {
            id: "entity.hero_pixl".to_string(),
            display_name: "Pixl Explorer".to_string(),
            archetype: EntityArchetype::Player,
            max_health: 100,
            move_speed: 64.0,
            hostile: false,
            sprite_id: "sprite_hero_pixl".to_string(),
            drop_table: vec![],
        })?;
        self.entities.register_or_override(EntityDefinition {
            id: "entity.mint_slime".to_string(),
            display_name: "Mint Slime".to_string(),
            archetype: EntityArchetype::Monster,
            max_health: 30,
            move_speed: 32.0,
            hostile: true,
            sprite_id: "sprite_mint_slime".to_string(),
            drop_table: vec!["item.berry_potion".to_string()],
        })?;

        self.tiles.register_or_override(TileDefinition {
            id: "tile.pastel_meadow".to_string(),
            display_name: "Pastel Meadow Grass".to_string(),
            walkable: true,
            transparent: true,
            movement_cost: 1.0,
            sprite_id: "tile_pastel_meadow".to_string(),
            custom_indices_8x8: None,
        })?;
        self.tiles.register_or_override(TileDefinition {
            id: "tile.pastel_cobble".to_string(),
            display_name: "Retro Cobblestone Wall".to_string(),
            walkable: false,
            transparent: false,
            movement_cost: 99.0,
            sprite_id: "tile_pastel_cobble".to_string(),
            custom_indices_8x8: None,
        })?;

        Ok(())
    }

    /// Total number of registered definitions across all 4 core registries.
    #[must_use]
    pub fn total_definitions(&self) -> usize {
        self.items.len() + self.classes.len() + self.entities.len() + self.tiles.len()
    }
}

/// Bevy Plugin that initializes `GameContentRegistries` and registers core defaults.
#[derive(Default)]
pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        let mut registries = GameContentRegistries::new();
        registries
            .register_core_defaults()
            .expect("Core default content definitions must be valid");
        app.insert_resource(registries);
    }
}
