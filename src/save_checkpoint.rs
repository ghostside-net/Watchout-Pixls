//! Auto-Save, Map Checkpoints, Death/Respawn Lifecycle, and Pause Menu Save/Load (Phase 4).
//!
//! Provides:
//! - [`GameSaveState`] & [`SaveGameManager`]: Full game state snapshot (player position,
//!   facing, [`CharacterProgression`], [`CombatStats`], [`ActiveCheckpoint`], and [`WorldSaveData`])
//!   supporting both in-memory save slots and on-disk RON/JSON persistence.
//! - [`AutoSaveConfig`]: Automatic background saves at fixed time/tick intervals and upon
//!   zone transitions (Overworld <-> Dungeon/Prefab instances).
//! - [`Checkpoint`] & [`ActiveCheckpoint`]: Map checkpoints that record the player's respawn
//!   location and automatically respawn/heal the player upon death.
//! - [`PauseMenuState`] & [`PauseMenuAction`]: Pause menu navigation with manual Save & Load.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::combat::{
    sync_character_stats_system, BossController, EnemyController, HealthPool,
    PlayerCombatActionEvent,
};
use crate::content::{DataFormat, GameContentRegistries};
use crate::interaction::{
    setup_phase3_overworld_props_system, InteractionOutcomeEvent, OverworldPropEntity,
};
use crate::palette_sprite::{IndexedPixelGrid, PixelSprite, WorldPixelPos};
use crate::player::{FacingDirection, PlayerInputState, PlayerMovement, PlayerState, YSort};
use crate::progression::{
    CharacterProgression, CombatStats, LevelUpEvent, SkillTree, WeaponEvolvedEvent,
};
use crate::scene::SceneManager;
use crate::state::{
    GameLoopMetrics, GameState, HeroController, InGameEntity, StateTransitionAction,
};
use crate::world::coords::WorldGridConfig;
use crate::world::noise::WorldSeed;
use crate::world::persistence::{WorldPersistence, WorldSaveData};
use crate::world::streaming::WorldMap;

/// Schema version for [`GameSaveState`] files.
pub const GAME_SAVE_STATE_VERSION: u32 = 1;

/// Default slot name used for automatic saves.
pub const AUTOSAVE_SLOT_NAME: &str = "autosave";

/// Default slot name used for manual saves from the Pause Menu.
pub const MANUAL_SAVE_SLOT_NAME: &str = "manual_1";

/// Errors that can occur when saving or loading a [`GameSaveState`].
#[derive(Debug, Error)]
pub enum SaveGameError {
    #[error("Save slot '{0}' was not found")]
    SlotNotFound(String),
    #[error("No Hero entity found in ECS world to capture or restore save state")]
    HeroNotFound,
    #[error("I/O error during savegame operation: {0}")]
    Io(#[from] std::io::Error),
    #[error("RON serialization error: {0}")]
    Ron(String),
    #[error("JSON serialization error: {0}")]
    Json(String),
    #[error("Unsupported savegame file extension: {0}")]
    UnsupportedFormat(String),
}

/// Reason why a game save was triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaveTriggerReason {
    /// Triggered automatically after the configured time/tick interval elapsed.
    AutoInterval,
    /// Triggered automatically when transitioning between zones/scenes (Overworld <-> Instance).
    ZoneTransition,
    /// Triggered automatically when activating a new map checkpoint.
    CheckpointActivated,
    /// Triggered manually by the user from the Pause Menu.
    ManualPauseMenu,
}

// ============================================================================
// 1. Checkpoint & Death/Respawn System
// ============================================================================

/// ECS Component representing a Checkpoint Shrine or Beacon on the map.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_id: String,
    pub display_name: String,
    pub activation_radius_px: f32,
    pub heal_on_activate: bool,
    pub is_activated: bool,
}

impl Checkpoint {
    #[must_use]
    pub fn new(id: impl Into<String>, display_name: impl Into<String>, radius_px: f32) -> Self {
        Self {
            checkpoint_id: id.into(),
            display_name: display_name.into(),
            activation_radius_px: radius_px.max(4.0),
            heal_on_activate: true,
            is_activated: false,
        }
    }
}

/// Resource tracking the player's currently active respawn checkpoint.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveCheckpoint {
    pub checkpoint_id: String,
    pub display_name: String,
    pub respawn_position: Vec2,
    pub in_overworld: bool,
    pub activation_count: u32,
    pub death_respawn_count: u32,
}

impl Default for ActiveCheckpoint {
    fn default() -> Self {
        Self {
            checkpoint_id: "checkpoint.sanctuary_origin".to_string(),
            display_name: "Sanctuary Waystone".to_string(),
            respawn_position: Vec2::new(152.0, 112.0),
            in_overworld: true,
            activation_count: 1,
            death_respawn_count: 0,
        }
    }
}

/// Event emitted when the player activates a map [`Checkpoint`].
#[derive(Event, Debug, Clone, PartialEq)]
pub struct CheckpointActivatedEvent {
    pub checkpoint_id: String,
    pub respawn_position: Vec2,
}

/// Event emitted when the player's health drops to `<= 0`.
#[derive(Event, Debug, Clone, PartialEq)]
pub struct PlayerDeathEvent {
    pub death_position: Vec2,
}

/// Event emitted after the player is respawned at the [`ActiveCheckpoint`].
#[derive(Event, Debug, Clone, PartialEq)]
pub struct PlayerRespawnedEvent {
    pub checkpoint_id: String,
    pub respawn_position: Vec2,
    pub restored_health: i32,
}

// ============================================================================
// 2. Complete Game Save State & Auto-Save Configuration
// ============================================================================

/// Complete serializable snapshot of the player's progression, health, position, checkpoint,
/// and modified/explored world state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSaveState {
    pub version: u32,
    pub reason: SaveTriggerReason,
    pub saved_at_tick: u64,
    pub hero_position: Vec2,
    pub hero_facing: FacingDirection,
    pub in_instance_id: Option<String>,
    pub progression: CharacterProgression,
    pub combat_stats: CombatStats,
    pub active_checkpoint: ActiveCheckpoint,
    pub world_save: WorldSaveData,
}

impl GameSaveState {
    /// Serializes this [`GameSaveState`] into a RON or JSON string.
    pub fn to_string_pretty(&self, format: DataFormat) -> Result<String, SaveGameError> {
        match format {
            DataFormat::Ron => {
                #[cfg(feature = "ron-content")]
                {
                    let pretty = ron::ser::PrettyConfig::default();
                    ron::ser::to_string_pretty(self, pretty)
                        .map_err(|e| SaveGameError::Ron(e.to_string()))
                }
                #[cfg(not(feature = "ron-content"))]
                {
                    Err(SaveGameError::UnsupportedFormat("ron".to_string()))
                }
            }
            DataFormat::Json => {
                #[cfg(feature = "json-content")]
                {
                    serde_json::to_string_pretty(self)
                        .map_err(|e| SaveGameError::Json(e.to_string()))
                }
                #[cfg(not(feature = "json-content"))]
                {
                    Err(SaveGameError::UnsupportedFormat("json".to_string()))
                }
            }
        }
    }

    /// Deserializes a [`GameSaveState`] from a RON or JSON string.
    pub fn from_str_format(raw: &str, format: DataFormat) -> Result<Self, SaveGameError> {
        match format {
            DataFormat::Ron => {
                #[cfg(feature = "ron-content")]
                {
                    ron::from_str(raw).map_err(|e| SaveGameError::Ron(e.to_string()))
                }
                #[cfg(not(feature = "ron-content"))]
                {
                    let _ = raw;
                    Err(SaveGameError::UnsupportedFormat("ron".to_string()))
                }
            }
            DataFormat::Json => {
                #[cfg(feature = "json-content")]
                {
                    serde_json::from_str(raw).map_err(|e| SaveGameError::Json(e.to_string()))
                }
                #[cfg(not(feature = "json-content"))]
                {
                    let _ = raw;
                    Err(SaveGameError::UnsupportedFormat("json".to_string()))
                }
            }
        }
    }

    /// Saves this [`GameSaveState`] to disk at `path` (inferring `.ron` or `.json` from extension).
    pub fn save_to_file(&self, path: impl AsRef<Path>) -> Result<usize, SaveGameError> {
        let path = path.as_ref();
        let format = DataFormat::from_path(path)
            .map_err(|e| SaveGameError::UnsupportedFormat(e.to_string()))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let encoded = self.to_string_pretty(format)?;
        fs::write(path, encoded.as_bytes())?;
        Ok(encoded.len())
    }

    /// Loads a [`GameSaveState`] from disk at `path`.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, SaveGameError> {
        let path = path.as_ref();
        let format = DataFormat::from_path(path)
            .map_err(|e| SaveGameError::UnsupportedFormat(e.to_string()))?;
        let raw = fs::read_to_string(path)?;
        Self::from_str_format(&raw, format)
    }
}

/// Configuration and runtime timer state for the background Auto-Save system.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutoSaveConfig {
    pub enabled: bool,
    /// Fixed interval in seconds between automatic saves during `GameState::InGame`.
    pub interval_secs: f32,
    /// Fixed interval in game ticks between automatic saves.
    pub interval_ticks: u64,
    /// Whether to automatically save whenever transitioning between Overworld and an Instance.
    pub save_on_zone_change: bool,
    /// Whether to automatically save when activating a map checkpoint.
    pub save_on_checkpoint: bool,
    /// Accumulated seconds since the last auto-save.
    pub elapsed_secs: f32,
    /// Accumulated ticks since the last auto-save.
    pub ticks_since_last_save: u64,
    /// Optional file path where auto-saves are written on disk.
    pub auto_save_file_path: Option<PathBuf>,
}

impl Default for AutoSaveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 30.0,
            interval_ticks: 1800,
            save_on_zone_change: true,
            save_on_checkpoint: true,
            elapsed_secs: 0.0,
            ticks_since_last_save: 0,
            auto_save_file_path: None,
        }
    }
}

/// Bevy Resource managing save slots (`autosave`, `manual_1`, etc.) and save/load counters.
#[derive(Resource, Debug, Default, Clone)]
pub struct SaveGameManager {
    slots: BTreeMap<String, GameSaveState>,
    pub auto_interval_saves: u64,
    pub zone_transition_saves: u64,
    pub checkpoint_saves: u64,
    pub manual_saves: u64,
    pub manual_loads: u64,
    pub last_save_reason: Option<SaveTriggerReason>,
}

impl SaveGameManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores a [`GameSaveState`] into `slot` and increments the counter for `state.reason`.
    pub fn store_slot(&mut self, slot: impl Into<String>, state: GameSaveState) {
        self.last_save_reason = Some(state.reason);
        match state.reason {
            SaveTriggerReason::AutoInterval => self.auto_interval_saves += 1,
            SaveTriggerReason::ZoneTransition => self.zone_transition_saves += 1,
            SaveTriggerReason::CheckpointActivated => self.checkpoint_saves += 1,
            SaveTriggerReason::ManualPauseMenu => self.manual_saves += 1,
        }
        self.slots.insert(slot.into(), state);
    }

    /// Retrieves a stored [`GameSaveState`] by slot name.
    #[must_use]
    pub fn get_slot(&self, slot: &str) -> Option<&GameSaveState> {
        self.slots.get(slot)
    }

    /// Returns `true` if `slot` exists in memory.
    #[must_use]
    pub fn has_slot(&self, slot: &str) -> bool {
        self.slots.contains_key(slot)
    }

    /// Total number of saves performed across all triggers.
    #[must_use]
    pub const fn total_saves(&self) -> u64 {
        self.auto_interval_saves
            + self.zone_transition_saves
            + self.checkpoint_saves
            + self.manual_saves
    }
}

/// Event emitted whenever a game save completes (automatically or manually).
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct GameSavedEvent {
    pub slot: String,
    pub reason: SaveTriggerReason,
    pub saved_at_tick: u64,
}

/// Event emitted whenever a game state is loaded and applied to the world.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct GameLoadedEvent {
    pub slot: String,
    pub restored_level: u32,
}

// ============================================================================
// 3. Pause Menu with Manual Save & Load
// ============================================================================

/// Selectable options in the in-game Pause Menu (`GameState::Paused`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PauseMenuOption {
    Resume,
    ManualSave,
    ManualLoad,
    ReturnToMainMenu,
}

/// Resource tracking the Pause Menu cursor, active save slot, and status feedback.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseMenuState {
    pub options: Vec<PauseMenuOption>,
    pub selected_index: usize,
    pub active_slot: String,
    pub save_file_path: Option<PathBuf>,
    pub status_message: String,
}

impl Default for PauseMenuState {
    fn default() -> Self {
        Self {
            options: vec![
                PauseMenuOption::Resume,
                PauseMenuOption::ManualSave,
                PauseMenuOption::ManualLoad,
                PauseMenuOption::ReturnToMainMenu,
            ],
            selected_index: 0,
            active_slot: MANUAL_SAVE_SLOT_NAME.to_string(),
            save_file_path: None,
            status_message: "PAUSED".to_string(),
        }
    }
}

impl PauseMenuState {
    /// Returns the currently highlighted [`PauseMenuOption`].
    #[must_use]
    pub fn selected_option(&self) -> PauseMenuOption {
        self.options
            .get(self.selected_index)
            .copied()
            .unwrap_or(PauseMenuOption::Resume)
    }

    /// Moves the menu cursor up or down with wrap-around.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.options.is_empty() {
            return;
        }
        let len = self.options.len() as i32;
        let next = (self.selected_index as i32 + delta).rem_euclid(len);
        self.selected_index = next as usize;
    }
}

/// Events for interacting with the Pause Menu (`GameState::Paused`) or triggering manual save/load.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub enum PauseMenuAction {
    /// Moves the pause menu selection cursor by `delta` (`-1` for up, `+1` for down).
    MoveCursor(i32),
    /// Executes the currently highlighted [`PauseMenuOption`].
    ConfirmSelection,
    /// Explicitly saves the current game state into `slot` (and optionally to `file_path`).
    ManualSave {
        slot: String,
        file_path: Option<PathBuf>,
    },
    /// Explicitly loads the game state from `slot` (or from `file_path` if specified).
    ManualLoad {
        slot: String,
        file_path: Option<PathBuf>,
    },
}

// ============================================================================
// 4. Snapshot Capture & Restoration Helpers
// ============================================================================

/// Helper that builds a [`GameSaveState`] from current ECS resources and Hero components.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn capture_game_save_state(
    reason: SaveTriggerReason,
    saved_at_tick: u64,
    hero_pos: WorldPixelPos,
    hero_facing: FacingDirection,
    progression: &CharacterProgression,
    combat_stats: &CombatStats,
    active_checkpoint: &ActiveCheckpoint,
    scene_manager: &SceneManager,
    world_seed: WorldSeed,
    grid_config: WorldGridConfig,
    world_map: &WorldMap,
    world_persistence: &WorldPersistence,
) -> GameSaveState {
    let in_instance_id = scene_manager
        .current_instance()
        .map(|inst| inst.instance_id.clone());
    let world_save =
        world_persistence.build_save_data(world_seed, grid_config, Some(world_map.active_chunks()));

    GameSaveState {
        version: GAME_SAVE_STATE_VERSION,
        reason,
        saved_at_tick,
        hero_position: hero_pos.to_vec2(),
        hero_facing,
        in_instance_id,
        progression: progression.clone(),
        combat_stats: *combat_stats,
        active_checkpoint: active_checkpoint.clone(),
        world_save,
    }
}

/// System that checks if the player stepped within range of any map [`Checkpoint`]
/// and activates it, updating [`ActiveCheckpoint`] and optionally triggering a checkpoint auto-save.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn checkpoint_activation_system(
    metrics: Res<GameLoopMetrics>,
    scene_manager: Res<SceneManager>,
    auto_save_config: Res<AutoSaveConfig>,
    world_seed: Res<WorldSeed>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    world_persistence: Res<WorldPersistence>,
    mut active_checkpoint: ResMut<ActiveCheckpoint>,
    mut save_manager: ResMut<SaveGameManager>,
    mut checkpoint_writer: EventWriter<CheckpointActivatedEvent>,
    mut saved_writer: EventWriter<GameSavedEvent>,
    mut hero_query: Query<
        (
            &WorldPixelPos,
            &FacingDirection,
            &CharacterProgression,
            &mut CombatStats,
        ),
        With<HeroController>,
    >,
    mut checkpoints_query: Query<(
        &WorldPixelPos,
        &mut Checkpoint,
        Option<&OverworldPropEntity>,
    )>,
) {
    let Some((hero_pos, hero_facing, progression, mut stats)) = hero_query.iter_mut().next() else {
        return;
    };
    if !stats.is_alive() {
        return;
    }

    let hero_vec = hero_pos.to_vec2();
    let in_instance = scene_manager.is_instance();
    for (cp_pos, mut cp, is_overworld_prop) in &mut checkpoints_query {
        if in_instance && is_overworld_prop.is_some() {
            continue;
        }
        let dist = hero_vec.distance(cp_pos.to_vec2());
        if dist <= cp.activation_radius_px && active_checkpoint.checkpoint_id != cp.checkpoint_id {
            cp.is_activated = true;
            active_checkpoint.checkpoint_id = cp.checkpoint_id.clone();
            active_checkpoint.display_name = cp.display_name.clone();
            active_checkpoint.respawn_position = cp_pos.to_vec2();
            active_checkpoint.in_overworld = scene_manager.is_overworld();
            active_checkpoint.activation_count += 1;

            if cp.heal_on_activate {
                stats.current_health = stats.max_health;
                stats.current_mana = stats.max_mana;
            }

            checkpoint_writer.send(CheckpointActivatedEvent {
                checkpoint_id: cp.checkpoint_id.clone(),
                respawn_position: cp_pos.to_vec2(),
            });

            if auto_save_config.enabled && auto_save_config.save_on_checkpoint {
                let snapshot = capture_game_save_state(
                    SaveTriggerReason::CheckpointActivated,
                    metrics.in_game_ticks,
                    *hero_pos,
                    *hero_facing,
                    progression,
                    &stats,
                    &active_checkpoint,
                    &scene_manager,
                    *world_seed,
                    *grid_config,
                    &world_map,
                    &world_persistence,
                );
                save_manager.store_slot(AUTOSAVE_SLOT_NAME, snapshot);
                saved_writer.send(GameSavedEvent {
                    slot: AUTOSAVE_SLOT_NAME.to_string(),
                    reason: SaveTriggerReason::CheckpointActivated,
                    saved_at_tick: metrics.in_game_ticks,
                });
            }
        }
    }
}

/// System that detects player death (`CombatStats.current_health <= 0` or [`PlayerDeathEvent`])
/// and respawns the player at the [`ActiveCheckpoint`] with full health.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn player_death_and_respawn_system(
    mut death_events: EventReader<PlayerDeathEvent>,
    mut respawn_writer: EventWriter<PlayerRespawnedEvent>,
    mut active_checkpoint: ResMut<ActiveCheckpoint>,
    mut scene_manager: ResMut<SceneManager>,
    mut hero_query: Query<
        (
            &mut WorldPixelPos,
            &mut CombatStats,
            &mut PlayerState,
            Option<&mut PlayerMovement>,
        ),
        With<HeroController>,
    >,
) {
    let explicit_death = death_events.read().next().is_some();
    death_events.clear();

    let Some((mut hero_pos, mut stats, mut player_state, opt_movement)) =
        hero_query.iter_mut().next()
    else {
        return;
    };

    if !explicit_death && stats.is_alive() {
        return;
    }

    // If the checkpoint is in the Overworld and the player died inside an instance, exit to Overworld
    if active_checkpoint.in_overworld && scene_manager.is_instance() {
        let _ = scene_manager.exit_to_overworld();
    }

    let respawn_pos = active_checkpoint.respawn_position;
    *hero_pos = WorldPixelPos::new(respawn_pos.x, respawn_pos.y);
    stats.current_health = stats.max_health.max(1);
    stats.current_mana = stats.max_mana;
    stats.invulnerability_timer = 1.0; // 1s spawn protection
    stats.basic_attack_cooldown = 0.0;
    stats.dash_cooldown = 0.0;
    stats.aoe_cooldown = 0.0;
    *player_state = PlayerState::Idle;

    if let Some(mut mov) = opt_movement {
        mov.last_applied_delta = Vec2::ZERO;
    }

    active_checkpoint.death_respawn_count += 1;
    respawn_writer.send(PlayerRespawnedEvent {
        checkpoint_id: active_checkpoint.checkpoint_id.clone(),
        respawn_position: respawn_pos,
        restored_health: stats.current_health,
    });
}

/// System that performs automatic background saves at fixed intervals and upon zone transitions
/// (`EnteredInstance` / `ExitedToOverworld`), as well as awarding chest loot to `CharacterProgression`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn auto_save_and_interaction_rewards_system(
    input: Res<PlayerInputState>,
    metrics: Res<GameLoopMetrics>,
    mut interaction_outcomes: EventReader<InteractionOutcomeEvent>,
    mut auto_save_config: ResMut<AutoSaveConfig>,
    mut save_manager: ResMut<SaveGameManager>,
    mut saved_writer: EventWriter<GameSavedEvent>,
    active_checkpoint: Res<ActiveCheckpoint>,
    scene_manager: Res<SceneManager>,
    world_seed: Res<WorldSeed>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    world_persistence: Res<WorldPersistence>,
    mut hero_query: Query<
        (
            &WorldPixelPos,
            &FacingDirection,
            &mut CharacterProgression,
            &CombatStats,
        ),
        With<HeroController>,
    >,
) {
    let Some((hero_pos, hero_facing, mut progression, stats)) = hero_query.iter_mut().next() else {
        interaction_outcomes.clear();
        return;
    };

    let mut zone_transition_occurred = false;
    for outcome in interaction_outcomes.read() {
        match outcome {
            InteractionOutcomeEvent::EnteredInstance { .. }
            | InteractionOutcomeEvent::ExitedToOverworld => {
                zone_transition_occurred = true;
            }
            InteractionOutcomeEvent::ChestOpened {
                chest_id: _,
                loot_item_id,
            } => {
                if !progression.inventory.contains(loot_item_id) {
                    progression.inventory.push(loot_item_id.clone());
                }
                let _ = progression.gain_xp(25);
            }
            InteractionOutcomeEvent::NpcDialogue { .. } => {}
        }
    }

    if !auto_save_config.enabled {
        return;
    }

    auto_save_config.elapsed_secs += input.delta_time.max(0.0);
    auto_save_config.ticks_since_last_save += 1;

    let interval_reached = (auto_save_config.interval_secs > 0.0
        && auto_save_config.elapsed_secs >= auto_save_config.interval_secs)
        || (auto_save_config.interval_ticks > 0
            && auto_save_config.ticks_since_last_save >= auto_save_config.interval_ticks);

    let trigger_reason = if zone_transition_occurred && auto_save_config.save_on_zone_change {
        Some(SaveTriggerReason::ZoneTransition)
    } else if interval_reached {
        Some(SaveTriggerReason::AutoInterval)
    } else {
        None
    };

    if let Some(reason) = trigger_reason {
        auto_save_config.elapsed_secs = 0.0;
        auto_save_config.ticks_since_last_save = 0;

        let snapshot = capture_game_save_state(
            reason,
            metrics.in_game_ticks,
            *hero_pos,
            *hero_facing,
            &progression,
            stats,
            &active_checkpoint,
            &scene_manager,
            *world_seed,
            *grid_config,
            &world_map,
            &world_persistence,
        );

        if let Some(ref path) = auto_save_config.auto_save_file_path {
            let _ = snapshot.save_to_file(path);
        }

        save_manager.store_slot(AUTOSAVE_SLOT_NAME, snapshot);
        saved_writer.send(GameSavedEvent {
            slot: AUTOSAVE_SLOT_NAME.to_string(),
            reason,
            saved_at_tick: metrics.in_game_ticks,
        });
    }
}

/// Resource caching the Hero's latest state so the Pause Menu (`GameState::Paused`)
/// can inspect, save, and restore player state seamlessly across `InGame <-> Paused` transitions.
#[derive(Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LivePlayerSnapshot {
    pub has_active_session: bool,
    pub hero_position: Vec2,
    pub hero_facing: FacingDirection,
    pub progression: CharacterProgression,
    pub combat_stats: CombatStats,
}

impl Default for LivePlayerSnapshot {
    fn default() -> Self {
        Self {
            has_active_session: false,
            hero_position: Vec2::new(152.0, 112.0),
            hero_facing: FacingDirection::South,
            progression: CharacterProgression::default(),
            combat_stats: CombatStats::default(),
        }
    }
}

/// Resets [`LivePlayerSnapshot`] when returning to `GameState::MainMenu`.
pub fn reset_live_snapshot_on_main_menu_system(mut snapshot: ResMut<LivePlayerSnapshot>) {
    snapshot.has_active_session = false;
}

/// Keeps [`LivePlayerSnapshot`] synchronized with the active Hero entity during `GameState::InGame`.
#[allow(clippy::type_complexity)]
pub fn sync_live_player_snapshot_system(
    mut snapshot: ResMut<LivePlayerSnapshot>,
    hero_query: Query<
        (
            &WorldPixelPos,
            &FacingDirection,
            &CharacterProgression,
            &CombatStats,
        ),
        With<HeroController>,
    >,
) {
    if let Some((pos, facing, prog, stats)) = hero_query.iter().next() {
        snapshot.has_active_session = true;
        snapshot.hero_position = pos.to_vec2();
        snapshot.hero_facing = *facing;
        snapshot.progression = prog.clone();
        snapshot.combat_stats = *stats;
    }
}

/// System that processes [`PauseMenuAction`] events (navigation, manual Save, manual Load, Resume, MainMenu).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn pause_menu_system(
    mut menu_actions: EventReader<PauseMenuAction>,
    mut pause_menu: ResMut<PauseMenuState>,
    mut save_manager: ResMut<SaveGameManager>,
    mut live_snapshot: ResMut<LivePlayerSnapshot>,
    mut saved_writer: EventWriter<GameSavedEvent>,
    mut loaded_writer: EventWriter<GameLoadedEvent>,
    mut transition_writer: EventWriter<StateTransitionAction>,
    metrics: Res<GameLoopMetrics>,
    mut active_checkpoint: ResMut<ActiveCheckpoint>,
    scene_manager: Res<SceneManager>,
    mut world_seed: ResMut<WorldSeed>,
    grid_config: Res<WorldGridConfig>,
    mut world_map: ResMut<WorldMap>,
    mut world_persistence: ResMut<WorldPersistence>,
    mut hero_query: Query<
        (
            &mut WorldPixelPos,
            &mut FacingDirection,
            &mut CharacterProgression,
            &mut CombatStats,
        ),
        With<HeroController>,
    >,
) {
    // Sync snapshot from live hero if present
    if let Some((pos, facing, prog, stats)) = hero_query.iter().next() {
        live_snapshot.has_active_session = true;
        live_snapshot.hero_position = pos.to_vec2();
        live_snapshot.hero_facing = *facing;
        live_snapshot.progression = prog.clone();
        live_snapshot.combat_stats = *stats;
    }

    for action in menu_actions.read() {
        match action {
            PauseMenuAction::MoveCursor(delta) => {
                pause_menu.move_cursor(*delta);
            }
            PauseMenuAction::ConfirmSelection => match pause_menu.selected_option() {
                PauseMenuOption::Resume => {
                    transition_writer.send(StateTransitionAction::TogglePause);
                }
                PauseMenuOption::ReturnToMainMenu => {
                    transition_writer.send(StateTransitionAction::ReturnToMainMenu);
                }
                PauseMenuOption::ManualSave => {
                    let slot = pause_menu.active_slot.clone();
                    let file_path = pause_menu.save_file_path.clone();
                    let snapshot = capture_game_save_state(
                        SaveTriggerReason::ManualPauseMenu,
                        metrics.in_game_ticks,
                        WorldPixelPos::new(
                            live_snapshot.hero_position.x,
                            live_snapshot.hero_position.y,
                        ),
                        live_snapshot.hero_facing,
                        &live_snapshot.progression,
                        &live_snapshot.combat_stats,
                        &active_checkpoint,
                        &scene_manager,
                        *world_seed,
                        *grid_config,
                        &world_map,
                        &world_persistence,
                    );
                    if let Some(ref path) = file_path {
                        let _ = snapshot.save_to_file(path);
                    }
                    save_manager.store_slot(slot.clone(), snapshot);
                    pause_menu.status_message = format!("Saved to {slot}");
                    saved_writer.send(GameSavedEvent {
                        slot,
                        reason: SaveTriggerReason::ManualPauseMenu,
                        saved_at_tick: metrics.in_game_ticks,
                    });
                }
                PauseMenuOption::ManualLoad => {
                    let slot = pause_menu.active_slot.clone();
                    let file_path = pause_menu.save_file_path.clone();
                    let loaded_opt = file_path
                        .as_ref()
                        .and_then(|p| GameSaveState::load_from_file(p).ok())
                        .or_else(|| save_manager.get_slot(&slot).cloned());

                    if let Some(save_state) = loaded_opt {
                        apply_loaded_save_state(
                            &save_state,
                            &mut save_manager,
                            &mut live_snapshot,
                            &mut active_checkpoint,
                            &mut world_seed,
                            &mut world_map,
                            &mut world_persistence,
                            &mut hero_query,
                        );
                        pause_menu.status_message = format!("Loaded {slot}");
                        loaded_writer.send(GameLoadedEvent {
                            slot,
                            restored_level: save_state.progression.level,
                        });
                    }
                }
            },
            PauseMenuAction::ManualSave { slot, file_path } => {
                let snapshot = capture_game_save_state(
                    SaveTriggerReason::ManualPauseMenu,
                    metrics.in_game_ticks,
                    WorldPixelPos::new(
                        live_snapshot.hero_position.x,
                        live_snapshot.hero_position.y,
                    ),
                    live_snapshot.hero_facing,
                    &live_snapshot.progression,
                    &live_snapshot.combat_stats,
                    &active_checkpoint,
                    &scene_manager,
                    *world_seed,
                    *grid_config,
                    &world_map,
                    &world_persistence,
                );
                if let Some(ref path) = file_path {
                    let _ = snapshot.save_to_file(path);
                }
                save_manager.store_slot(slot.clone(), snapshot);
                pause_menu.status_message = format!("Saved to {slot}");
                saved_writer.send(GameSavedEvent {
                    slot: slot.clone(),
                    reason: SaveTriggerReason::ManualPauseMenu,
                    saved_at_tick: metrics.in_game_ticks,
                });
            }
            PauseMenuAction::ManualLoad { slot, file_path } => {
                let loaded_opt = file_path
                    .as_ref()
                    .and_then(|p| GameSaveState::load_from_file(p).ok())
                    .or_else(|| save_manager.get_slot(slot).cloned());

                if let Some(save_state) = loaded_opt {
                    if !save_manager.has_slot(slot) {
                        save_manager.slots.insert(slot.clone(), save_state.clone());
                    }
                    apply_loaded_save_state(
                        &save_state,
                        &mut save_manager,
                        &mut live_snapshot,
                        &mut active_checkpoint,
                        &mut world_seed,
                        &mut world_map,
                        &mut world_persistence,
                        &mut hero_query,
                    );
                    pause_menu.status_message = format!("Loaded {slot}");
                    loaded_writer.send(GameLoadedEvent {
                        slot: slot.clone(),
                        restored_level: save_state.progression.level,
                    });
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn apply_loaded_save_state(
    save_state: &GameSaveState,
    save_manager: &mut SaveGameManager,
    live_snapshot: &mut LivePlayerSnapshot,
    active_checkpoint: &mut ActiveCheckpoint,
    world_seed: &mut WorldSeed,
    world_map: &mut WorldMap,
    world_persistence: &mut WorldPersistence,
    hero_query: &mut Query<
        (
            &mut WorldPixelPos,
            &mut FacingDirection,
            &mut CharacterProgression,
            &mut CombatStats,
        ),
        With<HeroController>,
    >,
) {
    save_manager.manual_loads += 1;
    *active_checkpoint = save_state.active_checkpoint.clone();
    *world_seed = save_state.world_save.seed;
    world_persistence.restore_from_save_data(save_state.world_save.clone());

    for chunk in &save_state.world_save.modified_chunks {
        if let Some(existing) = world_map.get_chunk_mut(chunk.coord) {
            *existing = chunk.clone();
        }
    }

    live_snapshot.has_active_session = true;
    live_snapshot.hero_position = save_state.hero_position;
    live_snapshot.hero_facing = save_state.hero_facing;
    live_snapshot.progression = save_state.progression.clone();
    live_snapshot.combat_stats = save_state.combat_stats;

    if let Some((mut hero_pos, mut hero_facing, mut progression, mut stats)) =
        hero_query.iter_mut().next()
    {
        *hero_pos = WorldPixelPos::new(save_state.hero_position.x, save_state.hero_position.y);
        *hero_facing = save_state.hero_facing;
        *progression = save_state.progression.clone();
        *stats = save_state.combat_stats;
    }
}

/// Spawns Phase 4 components on the Hero (`CharacterProgression`, `CombatStats`),
/// map [`Checkpoint`] shrines, standard patrolling enemies (`EnemyController`),
/// and the multi-phase Endboss (`BossController`) when entering `GameState::InGame`.
#[allow(clippy::type_complexity)]
pub fn setup_phase4_gameplay_system(
    mut commands: Commands,
    registries: Res<GameContentRegistries>,
    skill_tree: Res<SkillTree>,
    mut live_snapshot: ResMut<LivePlayerSnapshot>,
    mut active_checkpoint: ResMut<ActiveCheckpoint>,
    mut hero_query: Query<
        (Entity, &mut WorldPixelPos, &mut FacingDirection),
        (With<HeroController>, Without<CombatStats>),
    >,
) {
    for (entity, mut pos, mut facing) in &mut hero_query {
        if live_snapshot.has_active_session {
            *pos = WorldPixelPos::new(live_snapshot.hero_position.x, live_snapshot.hero_position.y);
            *facing = live_snapshot.hero_facing;
            commands.entity(entity).insert((
                live_snapshot.progression.clone(),
                live_snapshot.combat_stats,
            ));
        } else {
            let progression = CharacterProgression::from_class("class.pixel_ranger", &registries)
                .unwrap_or_default();
            let effective = progression.compute_effective_stats(&registries, &skill_tree);
            let combat_stats = CombatStats::from_effective(effective);

            live_snapshot.has_active_session = true;
            live_snapshot.hero_position = pos.to_vec2();
            live_snapshot.hero_facing = *facing;
            live_snapshot.progression = progression.clone();
            live_snapshot.combat_stats = combat_stats;

            commands.entity(entity).insert((progression, combat_stats));

            active_checkpoint.checkpoint_id = "checkpoint.sanctuary_origin".to_string();
            active_checkpoint.display_name = "Sanctuary Waystone".to_string();
            active_checkpoint.respawn_position = pos.to_vec2();
            active_checkpoint.in_overworld = true;
        }
    }

    // 1. Spawn a discoverable Overworld Checkpoint Shrine at (96.0, 144.0) (tile (6, 9))
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        WorldPixelPos::new(96.0, 144.0),
        PixelSprite {
            grid: IndexedPixelGrid::pastel_meadow_tile_8x8(),
            z_order: 252,
        },
        YSort::new(8.0, 100),
        Checkpoint::new("checkpoint.west_shrine", "Western Meadow Waystone", 16.0),
    ));

    // 2. Spawn Standard Patrolling Enemy 1: Mint Slime at (48.0, 192.0) <-> (80.0, 192.0)
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        WorldPixelPos::new(48.0, 192.0),
        PixelSprite {
            grid: IndexedPixelGrid::pastel_meadow_tile_8x8(),
            z_order: 300,
        },
        YSort::new(8.0, 100),
        HealthPool::new(30),
        EnemyController::mint_slime(Vec2::new(48.0, 192.0), Vec2::new(80.0, 192.0)),
    ));

    // 3. Spawn Standard Patrolling Enemy 2: Cobalt Goblin Scout at (320.0, 192.0) <-> (352.0, 192.0)
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        WorldPixelPos::new(320.0, 192.0),
        PixelSprite {
            grid: IndexedPixelGrid::pastel_cobble_tile_8x8(),
            z_order: 300,
        },
        YSort::new(8.0, 100),
        HealthPool::new(48),
        EnemyController::cobalt_goblin(vec![
            Vec2::new(320.0, 192.0),
            Vec2::new(352.0, 192.0),
            Vec2::new(352.0, 224.0),
        ]),
    ));

    // 4. Spawn Endboss: Chrono-Lich Overlord at (448.0, 288.0) (tile (28, 18))
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        WorldPixelPos::new(448.0, 288.0),
        PixelSprite {
            grid: IndexedPixelGrid::pastel_cobble_tile_8x8(),
            z_order: 396,
        },
        YSort::new(8.0, 100),
        HealthPool::new(650),
        BossController::chrono_overlord(),
    ));
}

/// Bevy Plugin registering all Phase 4 systems, resources, and events:
/// - Data-driven classes, weapons, and weapon evolution
/// - XP, Leveling, Attribute allocation, and Branching Skilltree (`SkillTree`)
/// - Standard Enemy State-Machine AI (`Patrol`, `Chase`, `Attack`) & Multi-Phase Endboss (`BossController`)
/// - Auto-Save (fixed intervals + zone transitions), Map Checkpoints (`Checkpoint` / `ActiveCheckpoint`),
///   Respawn after death, and Pause Menu Manual Save/Load (`PauseMenuState`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Phase4GameplayPlugin;

impl Plugin for Phase4GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SkillTree>()
            .init_resource::<ActiveCheckpoint>()
            .init_resource::<AutoSaveConfig>()
            .init_resource::<SaveGameManager>()
            .init_resource::<PauseMenuState>()
            .init_resource::<LivePlayerSnapshot>()
            .add_event::<LevelUpEvent>()
            .add_event::<WeaponEvolvedEvent>()
            .add_event::<PlayerCombatActionEvent>()
            .add_event::<crate::combat::CombatOutcomeEvent>()
            .add_event::<CheckpointActivatedEvent>()
            .add_event::<PlayerDeathEvent>()
            .add_event::<PlayerRespawnedEvent>()
            .add_event::<GameSavedEvent>()
            .add_event::<GameLoadedEvent>()
            .add_event::<PauseMenuAction>()
            .add_systems(
                OnEnter(GameState::MainMenu),
                reset_live_snapshot_on_main_menu_system
                    .after(crate::state::setup_main_menu_scene_system),
            )
            .add_systems(
                OnEnter(GameState::InGame),
                setup_phase4_gameplay_system
                    .after(setup_phase3_overworld_props_system)
                    .before(crate::world::queue_chunk_streaming_system),
            )
            .add_systems(
                OnExit(GameState::InGame),
                sync_live_player_snapshot_system
                    .before(crate::world::persist_and_clear_active_chunks_on_exit_system)
                    .before(crate::state::cleanup_in_game_scene_system),
            )
            .add_systems(
                Update,
                (
                    sync_character_stats_system,
                    crate::combat::player_combat_and_skills_system,
                    crate::combat::enemy_and_boss_ai_system,
                    checkpoint_activation_system,
                    player_death_and_respawn_system,
                    auto_save_and_interaction_rewards_system,
                    sync_live_player_snapshot_system,
                )
                    .chain()
                    .after(crate::interaction::handle_scene_transitions_system)
                    .before(crate::player::update_y_sorting_system)
                    .run_if(in_state(GameState::InGame)),
            )
            .add_systems(
                Update,
                pause_menu_system.before(crate::state::handle_state_transitions_system),
            );
    }
}
