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

// ============================================================================
// 5. Weapon Content Definition & Evolution Paths (Phase 4)
// ============================================================================

/// Evolution tier of a weapon (from simple starter weapons to Endgame/Sci-Fi weapons).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum WeaponTier {
    /// Simple starter weapons (e.g. Rusty Dagger).
    #[default]
    Starter,
    /// Forged intermediate weapons (e.g. Iron Shortsword).
    Intermediate,
    /// Masterwork / elemental weapons (e.g. Cobalt Katana, Pulse Arc Blade).
    Advanced,
    /// Endgame / Sci-Fi weapons (e.g. Pastel Laser Sword, Quantum Staff).
    EndgameSciFi,
}

const fn default_required_level() -> u32 {
    1
}

/// Data-driven definition for a Weapon supporting linear and branching evolution paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponDefinition {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub tier: WeaponTier,
    pub base_damage: i32,
    pub attack_speed: f32,
    pub range_px: f32,
    #[serde(default)]
    pub crit_chance: f32,
    #[serde(default = "default_required_level")]
    pub required_level: u32,
    /// IDs of successor weapons this weapon can evolve into.
    #[serde(default)]
    pub evolves_to: Vec<String>,
    pub sprite_id: String,
}

impl ContentDefinition for WeaponDefinition {
    const CONTENT_KIND: &'static str = "Weapon";

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
        if self.base_damage <= 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "base_damage must be > 0".to_string(),
            });
        }
        if self.attack_speed <= 0.0 || !self.attack_speed.is_finite() {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "attack_speed must be > 0.0".to_string(),
            });
        }
        if self.range_px <= 0.0 || !self.range_px.is_finite() {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "range_px must be > 0.0".to_string(),
            });
        }
        if !(0.0..=1.0).contains(&self.crit_chance) {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "crit_chance must be within 0.0..=1.0".to_string(),
            });
        }
        if self.required_level == 0 {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "required_level must be >= 1".to_string(),
            });
        }
        if self.evolves_to.iter().any(|next_id| next_id == &self.id) {
            return Err(ContentError::ValidationFailed {
                content_kind: Self::CONTENT_KIND,
                id: self.id.clone(),
                reason: "weapon cannot evolve into itself".to_string(),
            });
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
    pub weapons: ContentRegistry<WeaponDefinition>,
}

impl GameContentRegistries {
    /// Creates a new empty set of registries.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Populates the registries with built-in core content definitions so the
    /// base game always has valid fallback items, classes, entities, tiles, and weapons.
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
        self.items.register_or_override(ItemDefinition {
            id: "item.rusty_dagger".to_string(),
            display_name: "Rusty Dagger".to_string(),
            category: ItemCategory::Weapon,
            max_stack: 1,
            value: 35,
            power_bonus: 6,
            sprite_id: "sprite_rusty_dagger".to_string(),
        })?;
        self.items.register_or_override(ItemDefinition {
            id: "item.laser_sword".to_string(),
            display_name: "Pastel Laser Sword".to_string(),
            category: ItemCategory::Weapon,
            max_stack: 1,
            value: 1200,
            power_bonus: 60,
            sprite_id: "sprite_laser_sword".to_string(),
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
        self.classes.register_or_override(ClassDefinition {
            id: "class.blade_vanguard".to_string(),
            display_name: "Blade Vanguard".to_string(),
            base_health: 125,
            base_mana: 30,
            base_speed: 62.0,
            starting_items: vec![
                "weapon.rusty_dagger".to_string(),
                "item.berry_potion".to_string(),
            ],
            passive_trait: "VanguardEdge".to_string(),
        })?;
        self.classes.register_or_override(ClassDefinition {
            id: "class.cyber_sentinel".to_string(),
            display_name: "Cyber Sentinel".to_string(),
            base_health: 140,
            base_mana: 50,
            base_speed: 58.0,
            starting_items: vec![
                "weapon.rusty_dagger".to_string(),
                "item.berry_potion".to_string(),
            ],
            passive_trait: "NanoAegis".to_string(),
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
        self.entities.register_or_override(EntityDefinition {
            id: "entity.cobalt_goblin".to_string(),
            display_name: "Cobalt Goblin Scout".to_string(),
            archetype: EntityArchetype::Monster,
            max_health: 48,
            move_speed: 42.0,
            hostile: true,
            sprite_id: "sprite_cobalt_goblin".to_string(),
            drop_table: vec!["item.berry_potion".to_string()],
        })?;
        self.entities.register_or_override(EntityDefinition {
            id: "entity.boss_chrono_overlord".to_string(),
            display_name: "Chrono-Lich Overlord".to_string(),
            archetype: EntityArchetype::Monster,
            max_health: 650,
            move_speed: 45.0,
            hostile: true,
            sprite_id: "sprite_boss_chrono_overlord".to_string(),
            drop_table: vec!["item.laser_sword".to_string()],
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

        // Core Weapon Evolution Chain: Rusty Dagger -> Iron Shortsword -> (Cobalt Katana / Pulse Arc Blade) -> Pastel Laser Sword
        let core_weapons = [
            WeaponDefinition {
                id: "weapon.rusty_dagger".to_string(),
                display_name: "Rostiger Dolch".to_string(),
                tier: WeaponTier::Starter,
                base_damage: 6,
                attack_speed: 1.4,
                range_px: 18.0,
                crit_chance: 0.05,
                required_level: 1,
                evolves_to: vec!["weapon.iron_shortsword".to_string()],
                sprite_id: "sprite_rusty_dagger".to_string(),
            },
            WeaponDefinition {
                id: "weapon.pastel_sword".to_string(),
                display_name: "Pastel Mint Blade".to_string(),
                tier: WeaponTier::Starter,
                base_damage: 8,
                attack_speed: 1.4,
                range_px: 20.0,
                crit_chance: 0.05,
                required_level: 1,
                evolves_to: vec!["weapon.iron_shortsword".to_string()],
                sprite_id: "sprite_pastel_sword".to_string(),
            },
            WeaponDefinition {
                id: "weapon.iron_shortsword".to_string(),
                display_name: "Geschmiedetes Kurzschwert".to_string(),
                tier: WeaponTier::Intermediate,
                base_damage: 16,
                attack_speed: 1.5,
                range_px: 22.0,
                crit_chance: 0.08,
                required_level: 2,
                evolves_to: vec![
                    "weapon.cobalt_katana".to_string(),
                    "weapon.arc_blade".to_string(),
                ],
                sprite_id: "sprite_iron_shortsword".to_string(),
            },
            WeaponDefinition {
                id: "weapon.cobalt_katana".to_string(),
                display_name: "Kobalt-Katana".to_string(),
                tier: WeaponTier::Advanced,
                base_damage: 30,
                attack_speed: 1.8,
                range_px: 26.0,
                crit_chance: 0.16,
                required_level: 4,
                evolves_to: vec!["weapon.laser_sword".to_string()],
                sprite_id: "sprite_cobalt_katana".to_string(),
            },
            WeaponDefinition {
                id: "weapon.arc_blade".to_string(),
                display_name: "Impuls-Energieklinge".to_string(),
                tier: WeaponTier::Advanced,
                base_damage: 34,
                attack_speed: 1.5,
                range_px: 28.0,
                crit_chance: 0.12,
                required_level: 4,
                evolves_to: vec!["weapon.laser_sword".to_string()],
                sprite_id: "sprite_arc_blade".to_string(),
            },
            WeaponDefinition {
                id: "weapon.laser_sword".to_string(),
                display_name: "Pastell-Laserschwert".to_string(),
                tier: WeaponTier::EndgameSciFi,
                base_damage: 62,
                attack_speed: 2.2,
                range_px: 34.0,
                crit_chance: 0.25,
                required_level: 5,
                evolves_to: vec![],
                sprite_id: "sprite_laser_sword".to_string(),
            },
        ];

        for weapon in core_weapons {
            self.weapons.register_or_override(weapon)?;
        }

        Ok(())
    }

    /// Returns all direct successor [`WeaponDefinition`]s that `weapon_id` can evolve into.
    #[must_use]
    pub fn next_weapon_evolutions(&self, weapon_id: &str) -> Vec<&WeaponDefinition> {
        let Some(current) = self.weapons.get(weapon_id) else {
            return Vec::new();
        };
        current
            .evolves_to
            .iter()
            .filter_map(|target_id| self.weapons.get(target_id))
            .collect()
    }

    /// Traces the primary evolution chain starting at `start_weapon_id` up to the final Endgame weapon.
    #[must_use]
    pub fn trace_weapon_evolution_path(&self, start_weapon_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current_id = start_weapon_id;
        while let Some(def) = self.weapons.get(current_id) {
            if chain.iter().any(|visited| visited == &def.id) {
                break;
            }
            chain.push(def.id.clone());
            if let Some(first_next) = def.evolves_to.first() {
                current_id = first_next.as_str();
            } else {
                break;
            }
        }
        chain
    }

    /// Validates that all `evolves_to` target IDs across registered weapons exist in `self.weapons`.
    pub fn validate_weapon_evolution_paths(&self) -> Result<(), ContentError> {
        for (id, weapon) in self.weapons.iter() {
            for target_id in &weapon.evolves_to {
                if self.weapons.get(target_id).is_none() {
                    return Err(ContentError::ValidationFailed {
                        content_kind: WeaponDefinition::CONTENT_KIND,
                        id: id.to_string(),
                        reason: format!("unknown evolution target weapon '{target_id}'"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Total number of registered definitions across all core registries.
    #[must_use]
    pub fn total_definitions(&self) -> usize {
        self.items.len()
            + self.classes.len()
            + self.entities.len()
            + self.tiles.len()
            + self.weapons.len()
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
