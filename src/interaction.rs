//! Interaction & Trigger System for Entrances, Doors, Chests, and NPCs (Phase 3).
//!
//! Provides:
//! - [`Interactable`] trait and [`InteractableTrigger`] ECS component.
//! - [`TriggerMode::Automatic`] (step-on tile trigger) vs. [`TriggerMode::Interactive`] (press `E` / `Space` while in range & facing).
//! - Overworld placement of both a **Static Prefab House** (`starter_house.ron`) and a **Procedural BSP Dungeon**
//!   entrance in the starter sanctuary, plus deterministic procedural entrance placement across chunks.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::content::GameContentRegistries;
use crate::palette_sprite::{IndexedPixelGrid, PixelSprite, WorldPixelPos};
use crate::player::{
    compute_normalized_movement_delta, resolve_subtile_movement, update_y_sorting_system,
    FacingDirection, PlayerHitbox, PlayerInputState, PlayerMovement, PlayerState, YSort,
};
use crate::scene::{
    generate_bsp_dungeon, BspDungeonConfig, DungeonState, InstancePropKind, OverworldState,
    PrefabLoader, SceneManager, SceneTransitionEvent,
};
use crate::state::{GameState, HeroController, InGameEntity};
use crate::world::coords::{ChunkCoord, TileCoord, TilePos, WorldGridConfig, WorldPos};
use crate::world::noise::WorldSeed;
use crate::world::streaming::WorldMap;

/// Distinguishes between automatic step-on triggers and manual key-activated triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerMode {
    /// Fires automatically when the player steps onto the trigger's tile / radius.
    Automatic,
    /// Requires pressing the interaction key (`E` or `Space`) while within range and facing the target.
    Interactive,
}

/// Core trait implemented by interactive world entities (doors, portals, chests, NPCs).
pub trait Interactable {
    /// Unique identifier of this interactable object.
    fn interact_id(&self) -> &str;

    /// Activation mode ([`TriggerMode::Automatic`] vs [`TriggerMode::Interactive`]).
    fn trigger_mode(&self) -> TriggerMode;

    /// Maximum interaction distance in world pixels.
    fn interaction_range_px(&self) -> f32;

    /// Whether the player must face toward the object to interact (for [`TriggerMode::Interactive`]).
    fn requires_facing(&self) -> bool;

    /// Whether this interactable can currently be triggered.
    fn is_enabled(&self) -> bool;
}

/// Specific behavior payload of an [`InteractableTrigger`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InteractableKind {
    /// Door to a static prefab building (e.g. house or shrine).
    HouseEntrance {
        instance_id: String,
        prefab_path: String,
        /// Offset from the entrance position where the player spawns upon returning to the Overworld.
        return_offset: Vec2,
    },
    /// Cave mouth or portal to a seed-generated procedural BSP dungeon.
    DungeonEntrance {
        instance_id: String,
        seed: u64,
        config: BspDungeonConfig,
        /// Offset from the entrance position where the player spawns upon returning to the Overworld.
        return_offset: Vec2,
    },
    /// Ladder, door, or portal inside an instance that returns the player to the Overworld.
    InstanceExit,
    /// Lootable treasure chest containing an item ID.
    TreasureChest {
        chest_id: String,
        loot_item_id: String,
        opened: bool,
    },
    /// NPC with dialogue lines.
    Npc {
        npc_id: String,
        display_name: String,
        dialogue: String,
    },
}

/// ECS Component representing an interactive or automatic trigger in the world or instance.
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractableTrigger {
    pub id: String,
    pub mode: TriggerMode,
    pub range_px: f32,
    pub require_facing: bool,
    pub enabled: bool,
    pub kind: InteractableKind,
}

impl Interactable for InteractableTrigger {
    fn interact_id(&self) -> &str {
        &self.id
    }

    fn trigger_mode(&self) -> TriggerMode {
        self.mode
    }

    fn interaction_range_px(&self) -> f32 {
        self.range_px
    }

    fn requires_facing(&self) -> bool {
        self.require_facing
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl InteractableTrigger {
    /// Creates an interactive House Entrance trigger (requires pressing `E` or `Space` at the door).
    #[must_use]
    pub fn house_door(
        id: impl Into<String>,
        prefab_path: impl Into<String>,
        return_offset: Vec2,
    ) -> Self {
        let id_str = id.into();
        Self {
            id: id_str.clone(),
            mode: TriggerMode::Interactive,
            range_px: 20.0,
            require_facing: true,
            enabled: true,
            kind: InteractableKind::HouseEntrance {
                instance_id: id_str,
                prefab_path: prefab_path.into(),
                return_offset,
            },
        }
    }

    /// Creates an automatic Dungeon/Cave Entrance trigger (activates upon stepping onto the entrance tile).
    #[must_use]
    pub fn dungeon_portal(
        id: impl Into<String>,
        seed: u64,
        config: BspDungeonConfig,
        return_offset: Vec2,
    ) -> Self {
        let id_str = id.into();
        Self {
            id: id_str.clone(),
            mode: TriggerMode::Automatic,
            range_px: 10.0,
            require_facing: false,
            enabled: true,
            kind: InteractableKind::DungeonEntrance {
                instance_id: id_str,
                seed,
                config,
                return_offset,
            },
        }
    }

    /// Creates an exit trigger inside an instance that returns the player to the Overworld.
    #[must_use]
    pub fn instance_exit(id: impl Into<String>, mode: TriggerMode) -> Self {
        Self {
            id: id.into(),
            mode,
            range_px: 16.0,
            require_facing: false,
            enabled: true,
            kind: InteractableKind::InstanceExit,
        }
    }

    /// Creates an interactive Treasure Chest trigger.
    #[must_use]
    pub fn treasure_chest(id: impl Into<String>, loot_item_id: impl Into<String>) -> Self {
        let id_str = id.into();
        Self {
            id: id_str.clone(),
            mode: TriggerMode::Interactive,
            range_px: 18.0,
            require_facing: true,
            enabled: true,
            kind: InteractableKind::TreasureChest {
                chest_id: id_str,
                loot_item_id: loot_item_id.into(),
                opened: false,
            },
        }
    }
}

/// Marker component for Overworld entrance/prop entities.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct OverworldPropEntity;

/// Marker component for entities spawned exclusively for the active instance.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct InstanceSceneEntity;

/// Event sent when the player presses the interaction key (`E` or `Space`).
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayerInteractEvent;

/// Event emitted when an interaction succeeds (chest opened, NPC spoken to, or entrance triggered).
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub enum InteractionOutcomeEvent {
    ChestOpened {
        chest_id: String,
        loot_item_id: String,
    },
    NpcDialogue {
        npc_id: String,
        speaker: String,
        text: String,
    },
    EnteredInstance {
        instance_id: String,
    },
    ExitedToOverworld,
}

/// Pure function evaluating whether a player at `player_pos` facing `player_facing` can trigger
/// `trigger` located at `target_pos`.
#[must_use]
pub fn can_trigger_interaction(
    player_pos: WorldPos,
    player_facing: FacingDirection,
    target_pos: WorldPos,
    trigger: &InteractableTrigger,
    interact_key_pressed: bool,
) -> bool {
    if !trigger.is_enabled() {
        return false;
    }

    let delta = target_pos.to_vec2() - player_pos.to_vec2();
    let dist = delta.length();
    if dist > trigger.interaction_range_px() {
        return false;
    }

    match trigger.trigger_mode() {
        TriggerMode::Automatic => true,
        TriggerMode::Interactive => {
            if !interact_key_pressed {
                return false;
            }
            if !trigger.requires_facing() || dist <= 6.0 {
                return true;
            }
            let dir_to_target = delta.normalize_or_zero();
            let facing_vec = player_facing.unit_vector();
            facing_vec.dot(dir_to_target) >= -0.05
        }
    }
}

/// Deterministically computes a procedural entrance placement (`TilePos`) for a given Overworld [`ChunkCoord`]
/// and [`WorldSeed`], returning `Some((tile_pos, is_house))` for chunks that host a point of interest.
#[must_use]
pub fn procedural_chunk_entrance_for_coord(
    chunk_coord: ChunkCoord,
    seed: WorldSeed,
    grid: &WorldGridConfig,
) -> Option<(TilePos, bool)> {
    if chunk_coord == ChunkCoord::ZERO {
        return None;
    }
    let hash = seed.hash_tile(chunk_coord.x, chunk_coord.y, 0xD00D_CAFE_1234_5678);
    if !hash.is_multiple_of(3) {
        return None;
    }
    let lx = 4 + ((hash >> 8) as u32 % (grid.chunk_size_tiles.saturating_sub(8).max(1)));
    let ly = 4 + ((hash >> 16) as u32 % (grid.chunk_size_tiles.saturating_sub(8).max(1)));
    let global = grid.chunk_and_local_to_tile(
        chunk_coord,
        crate::world::coords::LocalTileCoord::new(lx, ly),
    );
    let is_house = ((hash >> 24) & 1) == 0;
    Some((TilePos::from(global), is_house))
}

/// Spawns the Overworld starter entrances (Prefab House at tile `(11, 6)` and Procedural BSP Dungeon
/// Cave at tile `(7, 6)`) plus an interactive Treasure Chest at `(9, 5)` when entering `GameState::InGame`.
pub fn setup_phase3_overworld_props_system(
    mut commands: Commands,
    seed: Res<WorldSeed>,
    grid: Res<WorldGridConfig>,
    mut hero_query: Query<(Entity, &HeroController), Without<PlayerMovement>>,
) {
    // Attach Phase 3 Player Controller components to the Hero entity if not already attached
    for (entity, hero) in &mut hero_query {
        commands.entity(entity).insert((
            PlayerMovement::new(hero.speed_px_per_sec),
            PlayerHitbox::default(),
            PlayerState::Idle,
            FacingDirection::South,
            YSort::new(8.0, 100),
        ));
    }

    // 1. Spawn Betretbares Haus (Prefab Cottage) at sanctuary tile (11, 6) -> world (176.0, 96.0)
    let house_tile = TileCoord::new(11, 6);
    let house_px = grid.tile_to_world_pixel(house_tile);
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        house_px,
        PixelSprite {
            grid: IndexedPixelGrid::pastel_cobble_tile_8x8(),
            z_order: compute_prop_z(house_px),
        },
        YSort::new(8.0, 100),
        InteractableTrigger::house_door(
            "entrance.starter_house",
            "assets/prefabs/starter_house.ron",
            Vec2::new(0.0, 16.0), // Return 1 tile south (in front of the door)
        ),
    ));

    // 2. Spawn Höhle / Prozeduraler BSP-Dungeon at sanctuary tile (7, 6) -> world (112.0, 96.0)
    let dungeon_tile = TileCoord::new(7, 6);
    let dungeon_px = grid.tile_to_world_pixel(dungeon_tile);
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        dungeon_px,
        PixelSprite {
            grid: IndexedPixelGrid::pastel_cobble_tile_8x8(),
            z_order: compute_prop_z(dungeon_px),
        },
        YSort::new(8.0, 100),
        InteractableTrigger::dungeon_portal(
            "entrance.starter_cavern",
            seed.0 ^ 0xCA7E_0001,
            BspDungeonConfig::default(),
            Vec2::new(0.0, 16.0), // Return 1 tile south (outside the cave mouth)
        ),
    ));

    // 3. Spawn an interactive Overworld Treasure Chest at sanctuary tile (9, 5) -> world (144.0, 80.0)
    let chest_tile = TileCoord::new(9, 5);
    let chest_px = grid.tile_to_world_pixel(chest_tile);
    commands.spawn((
        InGameEntity,
        OverworldPropEntity,
        chest_px,
        PixelSprite {
            grid: IndexedPixelGrid::pastel_meadow_tile_8x8(),
            z_order: compute_prop_z(chest_px),
        },
        YSort::new(8.0, 100),
        InteractableTrigger::treasure_chest("chest.sanctuary_01", "item.pastel_sword"),
    ));
}

fn compute_prop_z(pos: WorldPixelPos) -> i32 {
    100 + (pos.y + 8.0).round() as i32
}

/// System that processes continuous 8-way player movement from [`PlayerInputState`]
/// with diagonal normalization and sub-tile AABB sliding collision against either
/// the Overworld [`WorldMap`] or the active [`DungeonState`] instance.
#[allow(clippy::type_complexity)]
pub fn player_movement_system(
    mut input: ResMut<PlayerInputState>,
    grid_config: Res<WorldGridConfig>,
    world_map: Res<WorldMap>,
    mut scene_manager: ResMut<SceneManager>,
    mut player_query: Query<
        (
            &mut WorldPixelPos,
            &mut PlayerMovement,
            &PlayerHitbox,
            &mut PlayerState,
            &mut FacingDirection,
        ),
        With<HeroController>,
    >,
) {
    if scene_manager.transition_cooldown_ticks > 0 {
        scene_manager.transition_cooldown_ticks -= 1;
    }

    let Some((mut pos, mut movement, hitbox, mut state, mut facing)) =
        player_query.iter_mut().next()
    else {
        return;
    };

    if !state.can_move() {
        movement.last_applied_delta = Vec2::ZERO;
        return;
    }

    let raw_dir = input.raw_direction();
    movement.input_dir = raw_dir;

    if raw_dir == Vec2::ZERO {
        movement.last_applied_delta = Vec2::ZERO;
        if *state == PlayerState::Moving {
            *state = PlayerState::Idle;
        }
        return;
    }

    if let Some(new_facing) = FacingDirection::from_vec2(raw_dir) {
        *facing = new_facing;
    }

    let desired_delta =
        compute_normalized_movement_delta(raw_dir, movement.speed_px_per_sec, input.delta_time);

    let current_world_pos = WorldPos::from(*pos);
    let (new_world_pos, applied_delta) = match scene_manager.current_instance() {
        Some(dungeon) => resolve_subtile_movement(
            current_world_pos,
            hitbox,
            desired_delta,
            dungeon,
            &grid_config,
        ),
        None => resolve_subtile_movement(
            current_world_pos,
            hitbox,
            desired_delta,
            &*world_map,
            &grid_config,
        ),
    };

    *pos = new_world_pos.to_world_pixel_pos();
    movement.last_applied_delta = applied_delta;
    *state = if applied_delta.length_squared() > 0.0 {
        PlayerState::Moving
    } else {
        PlayerState::Idle
    };

    // Consume single-frame interaction press after evaluation in trigger system
    let _ = &mut input;
}

/// System that evaluates [`InteractableTrigger`] entities and instance exit triggers
/// for both [`TriggerMode::Automatic`] (step-on) and [`TriggerMode::Interactive`] (`E` / `Space`).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn player_interaction_and_trigger_system(
    mut input: ResMut<PlayerInputState>,
    mut interact_events: EventReader<PlayerInteractEvent>,
    scene_manager: Res<SceneManager>,
    mut transition_writer: EventWriter<SceneTransitionEvent>,
    mut outcome_writer: EventWriter<InteractionOutcomeEvent>,
    mut player_query: Query<
        (&WorldPixelPos, &FacingDirection, &mut PlayerState),
        With<HeroController>,
    >,
    mut trigger_query: Query<(
        &WorldPixelPos,
        &mut InteractableTrigger,
        Option<&OverworldPropEntity>,
        Option<&InstanceSceneEntity>,
    )>,
) {
    let interact_pressed = input.interact_just_pressed || interact_events.read().next().is_some();
    input.interact_just_pressed = false;
    interact_events.clear();

    if scene_manager.transition_cooldown_ticks > 0 {
        return;
    }

    let Some((player_pos_px, player_facing, mut player_state)) = player_query.iter_mut().next()
    else {
        return;
    };

    if *player_state == PlayerState::Transitioning {
        return;
    }

    let player_pos = WorldPos::from(*player_pos_px);
    let in_instance = scene_manager.is_instance();

    // 1. If inside an instance, check if player stepped onto or interacted with the instance's exit_tile
    if let Some(dungeon) = scene_manager.current_instance() {
        let exit_world = dungeon.exit_world_pos();
        let dist_to_exit = player_pos.distance(exit_world);
        if dist_to_exit <= 12.0 && (interact_pressed || dist_to_exit <= 6.0) {
            *player_state = PlayerState::Transitioning;
            transition_writer.send(SceneTransitionEvent::ExitToOverworld);
            return;
        }
    }

    // 2. Evaluate ECS InteractableTrigger entities belonging to the active scene context
    for (trigger_pos_px, mut trigger, is_overworld_prop, is_instance_prop) in &mut trigger_query {
        if in_instance && is_overworld_prop.is_some() {
            continue;
        }
        if !in_instance && is_instance_prop.is_some() {
            continue;
        }

        let trigger_pos = WorldPos::from(*trigger_pos_px);
        let should_fire = match trigger.mode {
            TriggerMode::Automatic => {
                can_trigger_interaction(player_pos, *player_facing, trigger_pos, &trigger, false)
                    || (interact_pressed
                        && player_pos.distance(trigger_pos) <= trigger.range_px.max(18.0))
            }
            TriggerMode::Interactive => can_trigger_interaction(
                player_pos,
                *player_facing,
                trigger_pos,
                &trigger,
                interact_pressed,
            ),
        };

        if !should_fire {
            continue;
        }

        match &mut trigger.kind {
            InteractableKind::HouseEntrance {
                instance_id: _,
                prefab_path,
                return_offset,
            } => {
                *player_state = PlayerState::Transitioning;
                let return_pos = if *return_offset == Vec2::ZERO {
                    player_pos.to_vec2()
                } else {
                    trigger_pos.to_vec2() + *return_offset
                };
                transition_writer.send(SceneTransitionEvent::EnterPrefab {
                    prefab_path: prefab_path.clone(),
                    return_position: Some(return_pos),
                });
                break;
            }
            InteractableKind::DungeonEntrance {
                instance_id,
                seed,
                config,
                return_offset,
            } => {
                *player_state = PlayerState::Transitioning;
                let return_pos = if *return_offset == Vec2::ZERO {
                    player_pos.to_vec2()
                } else {
                    trigger_pos.to_vec2() + *return_offset
                };
                transition_writer.send(SceneTransitionEvent::EnterProceduralDungeon {
                    instance_id: instance_id.clone(),
                    seed: *seed,
                    config: *config,
                    return_position: Some(return_pos),
                });
                break;
            }
            InteractableKind::InstanceExit => {
                *player_state = PlayerState::Transitioning;
                transition_writer.send(SceneTransitionEvent::ExitToOverworld);
                break;
            }
            InteractableKind::TreasureChest {
                chest_id,
                loot_item_id,
                opened,
            } => {
                if !*opened {
                    *opened = true;
                    let cid = chest_id.clone();
                    let lid = loot_item_id.clone();
                    trigger.enabled = false;
                    *player_state = PlayerState::Interacting;
                    outcome_writer.send(InteractionOutcomeEvent::ChestOpened {
                        chest_id: cid,
                        loot_item_id: lid,
                    });
                    break;
                }
            }
            InteractableKind::Npc {
                npc_id,
                display_name,
                dialogue,
            } => {
                *player_state = PlayerState::Interacting;
                outcome_writer.send(InteractionOutcomeEvent::NpcDialogue {
                    npc_id: npc_id.clone(),
                    speaker: display_name.clone(),
                    text: dialogue.clone(),
                });
                break;
            }
        }
    }
}

/// System that executes [`SceneTransitionEvent`]s, saving/restoring the Overworld state
/// (`return_position`, `world_seed`, `cached_chunks`) and teleporting the Hero cleanly.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn handle_scene_transitions_system(
    mut commands: Commands,
    mut events: EventReader<SceneTransitionEvent>,
    mut outcome_writer: EventWriter<InteractionOutcomeEvent>,
    mut scene_manager: ResMut<SceneManager>,
    world_seed: Res<WorldSeed>,
    world_map: Res<WorldMap>,
    registries: Option<Res<GameContentRegistries>>,
    mut player_query: Query<
        (&mut WorldPixelPos, &mut FacingDirection, &mut PlayerState),
        With<HeroController>,
    >,
    instance_entities: Query<Entity, With<InstanceSceneEntity>>,
) {
    let Some((mut player_pos, mut player_facing, mut player_state)) =
        player_query.iter_mut().next()
    else {
        events.clear();
        return;
    };

    for event in events.read() {
        match event {
            SceneTransitionEvent::EnterProceduralDungeon {
                instance_id,
                seed,
                config,
                return_position,
            } => {
                let instance = generate_bsp_dungeon(instance_id.clone(), *seed, config)
                    .unwrap_or_else(|_| {
                        generate_bsp_dungeon(
                            instance_id.clone(),
                            *seed,
                            &BspDungeonConfig::default(),
                        )
                        .expect("Default BspDungeonConfig is always >= 12x12")
                    });

                let overworld_snapshot = OverworldState {
                    return_position: return_position.unwrap_or_else(|| player_pos.to_vec2()),
                    return_facing: *player_facing,
                    world_seed: *world_seed,
                    cached_chunks: world_map.active_chunks().clone(),
                };

                // Clear any previous instance entities and spawn new ones
                for entity in &instance_entities {
                    commands.entity(entity).despawn();
                }
                spawn_instance_entities(&mut commands, &instance);

                let id_clone = instance.instance_id.clone();
                let spawn_pos = scene_manager.enter_instance(overworld_snapshot, instance);
                *player_pos = spawn_pos.to_world_pixel_pos();
                *player_state = PlayerState::Idle;
                outcome_writer.send(InteractionOutcomeEvent::EnteredInstance {
                    instance_id: id_clone,
                });
            }
            SceneTransitionEvent::EnterPrefab {
                prefab_path,
                return_position,
            } => {
                let instance =
                    PrefabLoader::load_from_file_or_fallback(prefab_path, registries.as_deref());
                let overworld_snapshot = OverworldState {
                    return_position: return_position.unwrap_or_else(|| player_pos.to_vec2()),
                    return_facing: *player_facing,
                    world_seed: *world_seed,
                    cached_chunks: world_map.active_chunks().clone(),
                };

                for entity in &instance_entities {
                    commands.entity(entity).despawn();
                }
                spawn_instance_entities(&mut commands, &instance);

                let id_clone = instance.instance_id.clone();
                let spawn_pos = scene_manager.enter_instance(overworld_snapshot, instance);
                *player_pos = spawn_pos.to_world_pixel_pos();
                *player_state = PlayerState::Idle;
                outcome_writer.send(InteractionOutcomeEvent::EnteredInstance {
                    instance_id: id_clone,
                });
            }
            SceneTransitionEvent::EnterInstance {
                instance,
                return_position,
            } => {
                let overworld_snapshot = OverworldState {
                    return_position: return_position.unwrap_or_else(|| player_pos.to_vec2()),
                    return_facing: *player_facing,
                    world_seed: *world_seed,
                    cached_chunks: world_map.active_chunks().clone(),
                };

                for entity in &instance_entities {
                    commands.entity(entity).despawn();
                }
                spawn_instance_entities(&mut commands, instance);

                let id_clone = instance.instance_id.clone();
                let spawn_pos = scene_manager.enter_instance(overworld_snapshot, instance.clone());
                *player_pos = spawn_pos.to_world_pixel_pos();
                *player_state = PlayerState::Idle;
                outcome_writer.send(InteractionOutcomeEvent::EnteredInstance {
                    instance_id: id_clone,
                });
            }
            SceneTransitionEvent::ExitToOverworld => {
                if let Some(restored) = scene_manager.exit_to_overworld() {
                    for entity in &instance_entities {
                        commands.entity(entity).despawn();
                    }
                    *player_pos =
                        WorldPixelPos::new(restored.return_position.x, restored.return_position.y);
                    *player_facing = restored.return_facing;
                    *player_state = PlayerState::Idle;
                    outcome_writer.send(InteractionOutcomeEvent::ExitedToOverworld);
                }
            }
        }
    }
}

fn spawn_instance_entities(commands: &mut Commands, instance: &DungeonState) {
    for prop in &instance.props {
        let world_pos = instance
            .tile_to_entity_world_pos(prop.tile)
            .to_world_pixel_pos();
        match &prop.kind {
            InstancePropKind::ExitPortal { label } => {
                commands.spawn((
                    InGameEntity,
                    InstanceSceneEntity,
                    world_pos,
                    InteractableTrigger::instance_exit(
                        format!("{}.exit_{}", instance.instance_id, label),
                        TriggerMode::Automatic,
                    ),
                ));
            }
            InstancePropKind::TreasureChest {
                chest_id,
                loot_item_id,
                opened: _,
            } => {
                commands.spawn((
                    InGameEntity,
                    InstanceSceneEntity,
                    world_pos,
                    InteractableTrigger::treasure_chest(chest_id.clone(), loot_item_id.clone()),
                ));
            }
            InstancePropKind::Npc {
                npc_id,
                display_name,
                dialogue,
            } => {
                commands.spawn((
                    InGameEntity,
                    InstanceSceneEntity,
                    world_pos,
                    InteractableTrigger {
                        id: npc_id.clone(),
                        mode: TriggerMode::Interactive,
                        range_px: 20.0,
                        require_facing: true,
                        enabled: true,
                        kind: InteractableKind::Npc {
                            npc_id: npc_id.clone(),
                            display_name: display_name.clone(),
                            dialogue: dialogue.clone(),
                        },
                    },
                ));
            }
            InstancePropKind::Decoration { .. } => {}
        }
    }
}

/// Bevy Plugin registering Phase 3 Player Controller, Collision, Interaction, and Scene Management systems.
#[derive(Debug, Default, Clone, Copy)]
pub struct Phase3GameplayPlugin;

impl Plugin for Phase3GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerInputState>()
            .init_resource::<SceneManager>()
            .add_event::<PlayerInteractEvent>()
            .add_event::<InteractionOutcomeEvent>()
            .add_event::<SceneTransitionEvent>()
            .add_systems(
                OnEnter(GameState::InGame),
                setup_phase3_overworld_props_system.after(crate::state::setup_in_game_scene_system),
            )
            .add_systems(
                Update,
                (
                    player_movement_system,
                    player_interaction_and_trigger_system,
                    handle_scene_transitions_system,
                    update_y_sorting_system,
                )
                    .chain()
                    .run_if(in_state(GameState::InGame)),
            );
    }
}
