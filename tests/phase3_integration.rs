//! Integration tests for Phase 3 (Player Controller, Sub-Tile Sliding Collision,
//! Interactive Triggers, Procedural BSP Dungeons, and Static Prefab Instances).

use bevy::prelude::*;
use watchout_pixls::build_watchout_app;
use watchout_pixls::content::DataFormat;
use watchout_pixls::interaction::{
    InteractableKind, InteractableTrigger, PlayerInteractEvent, TriggerMode,
};
use watchout_pixls::palette_sprite::{PixelSprite, WorldPixelPos};
use watchout_pixls::player::{
    check_aabb_collision, compute_normalized_movement_delta, resolve_subtile_movement,
    FixedCollisionGrid, PlayerHitbox, PlayerInputState, PlayerState,
};
use watchout_pixls::scene::{
    generate_bsp_dungeon, BspDungeonConfig, PrefabError, PrefabLoader, SceneManager,
    SceneTransitionEvent,
};
use watchout_pixls::state::{GameState, HeroController, StateTransitionAction};
use watchout_pixls::world::{
    ChunkCoord, HeroMoveEvent, TileCollision, TileCoord, TilePos, WallKind, WorldGridConfig,
    WorldMap, WorldPos,
};

#[test]
fn test_scene_transition_lifecycle() {
    let mut app = build_watchout_app();

    // 1. Start in MainMenu -> transition into InGame (Overworld)
    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::InGame
    );
    assert!(app.world().resource::<SceneManager>().is_overworld());

    // Record exact return position in the Overworld before entering the dungeon
    let expected_return_pos = Vec2::new(112.0, 112.0);
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(expected_return_pos));
    app.update();

    let overworld_pos_before = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();
    assert_eq!(overworld_pos_before, expected_return_pos);

    // 2. Enter Procedural BSP Dungeon from the Overworld
    let bsp_config = BspDungeonConfig::default();
    let dungeon_seed = 0x1234_5678_9ABC_DEF0;
    app.world_mut()
        .send_event(SceneTransitionEvent::EnterProceduralDungeon {
            instance_id: "dungeon.lifecycle_test".to_string(),
            seed: dungeon_seed,
            config: bsp_config,
            return_position: Some(expected_return_pos),
        });
    app.update();

    // 3. Validate active scene is Scene::Instance(DungeonState) and Hero is at the dungeon's spawn point
    let expected_dungeon_spawn = {
        let scene_manager = app.world().resource::<SceneManager>();
        assert!(scene_manager.is_instance());
        let dungeon = scene_manager.current_instance().unwrap();
        assert_eq!(dungeon.instance_id, "dungeon.lifecycle_test");
        assert!(dungeon.is_connected_spawn_to_exit());
        dungeon.spawn_world_pos().to_vec2()
    };

    let hero_in_dungeon_pos = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();
    assert_eq!(hero_in_dungeon_pos, expected_dungeon_spawn);

    // Verify Overworld chunks remained cached/loaded while inside the dungeon
    assert_eq!(app.world().resource::<WorldMap>().active_chunk_count(), 9);

    // 4. Exit the Dungeon back to the Overworld
    app.world_mut()
        .send_event(SceneTransitionEvent::ExitToOverworld);
    app.update();

    // 5. Verify active scene is back to Scene::Overworld and Hero is at the exact return position
    {
        let scene_manager = app.world().resource::<SceneManager>();
        assert!(scene_manager.is_overworld());
        assert!(scene_manager.current_instance().is_none());
    }

    let hero_returned_pos = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();
    assert_eq!(hero_returned_pos, expected_return_pos);
}

#[test]
fn integration_overworld_house_prefab_and_dungeon_triggers() {
    let mut app = build_watchout_app();

    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    // Verify both the House Entrance (Prefab) and Dungeon Entrance (BSP) exist in the Overworld
    let mut has_house_entrance = false;
    let mut has_dungeon_entrance = false;
    for trigger in app
        .world_mut()
        .query::<&InteractableTrigger>()
        .iter(app.world())
    {
        match &trigger.kind {
            InteractableKind::HouseEntrance { .. } => {
                assert_eq!(trigger.mode, TriggerMode::Interactive);
                has_house_entrance = true;
            }
            InteractableKind::DungeonEntrance { .. } => {
                assert_eq!(trigger.mode, TriggerMode::Automatic);
                has_dungeon_entrance = true;
            }
            _ => {}
        }
    }
    assert!(
        has_house_entrance,
        "Overworld must contain at least one enterable House (Prefab)"
    );
    assert!(
        has_dungeon_entrance,
        "Overworld must contain at least one enterable Cave/Dungeon (BSP)"
    );

    // 1. Test Interactive House Entrance at (176.0, 96.0):
    // Position Hero right in front of the door at (176.0, 112.0) facing North and press E
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(Vec2::new(176.0, 112.0)));
    {
        let mut input = app.world_mut().resource_mut::<PlayerInputState>();
        input.up = true;
        input.delta_time = 0.01;
    }
    app.update(); // Faces North toward the house door

    {
        let mut input = app.world_mut().resource_mut::<PlayerInputState>();
        input.up = false;
        input.interact_just_pressed = true;
    }
    app.world_mut().send_event(PlayerInteractEvent);
    app.update(); // Processes interaction -> queues EnterPrefab
    app.update(); // Executes EnterPrefab transition

    {
        let scene_manager = app.world().resource::<SceneManager>();
        assert!(scene_manager.is_instance());
        let house = scene_manager.current_instance().unwrap();
        assert_eq!(house.instance_id, "prefab.starter_house");
    }

    // Step onto the house exit tile (or send ExitToOverworld) and verify return position
    let exit_px = app
        .world()
        .resource::<SceneManager>()
        .current_instance()
        .unwrap()
        .exit_world_pos()
        .to_vec2();
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(exit_px));
    app.update();
    app.update();
    app.update(); // Cooldown expires & automatic exit triggers -> queues ExitToOverworld
    app.update(); // Executes ExitToOverworld

    assert!(app.world().resource::<SceneManager>().is_overworld());
    let returned_from_house = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();
    // House is at (176.0, 96.0) with return_offset (0.0, 16.0) -> (176.0, 112.0)
    assert_eq!(returned_from_house, Vec2::new(176.0, 112.0));
}

#[test]
fn integration_chunk_boundary_collision_and_y_sorting() {
    let mut app = build_watchout_app();

    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    let grid = *app.world().resource::<WorldGridConfig>();

    // Place a solid wall at the chunk boundary: tile (16, 7) is local (0, 7) of chunk (1, 0),
    // while tile (15, 7) is local (15, 7) of chunk (0, 0).
    {
        let mut world_map = app.world_mut().resource_mut::<WorldMap>();
        assert!(world_map.set_wall(TileCoord::new(15, 7), &grid, None));
        assert!(world_map.set_wall(TileCoord::new(16, 7), &grid, Some(WallKind::CobbleWall)));
    }

    // Teleport Hero to tile (15, 7) at the eastern edge of chunk (0, 0)
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToTile(TileCoord::new(15, 7)));
    app.update();

    let start_px = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();
    assert_eq!(grid.world_pixel_to_chunk(start_px), ChunkCoord::new(0, 0));

    // Attempt to move East across the chunk boundary into the solid wall at tile (16, 7) in chunk (1, 0)
    {
        let mut input = app.world_mut().resource_mut::<PlayerInputState>();
        input.right = true;
        input.delta_time = 0.25; // 64 px/s * 0.25s = 16 px East
    }
    app.update();

    let blocked_px = app
        .world_mut()
        .query_filtered::<&WorldPixelPos, With<HeroController>>()
        .single(app.world())
        .to_vec2();

    // Must not penetrate into tile (16, 7) (which starts at x = 256.0)
    let hitbox = PlayerHitbox::default();
    assert!(
        hitbox.aabb_at(WorldPos::from(blocked_px)).max.x <= 256.0,
        "Player AABB must be stopped at the chunk boundary wall (x <= 256.0), got {}",
        hitbox.aabb_at(WorldPos::from(blocked_px)).max.x
    );

    // Clear input and verify PlayerState returns to Idle
    {
        let mut input = app.world_mut().resource_mut::<PlayerInputState>();
        input.clear();
    }
    app.update();

    let state = *app
        .world_mut()
        .query_filtered::<&PlayerState, With<HeroController>>()
        .single(app.world());
    assert_eq!(state, PlayerState::Idle);

    // Verify Y-Sorting updates Hero z_order based on Y coordinate
    let z_before = app
        .world_mut()
        .query_filtered::<&PixelSprite, With<HeroController>>()
        .single(app.world())
        .z_order;

    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToPixel(Vec2::new(152.0, 140.0)));
    app.update();

    let z_after = app
        .world_mut()
        .query_filtered::<&PixelSprite, With<HeroController>>()
        .single(app.world())
        .z_order;
    assert!(
        z_after > z_before,
        "Moving South (+Y) must increase Y-sorted z_order ({z_after} > {z_before})"
    );
}

#[test]
fn test_player_diagonal_movement_normalization() {
    let delta_cardinal = compute_normalized_movement_delta(Vec2::new(1.0, 0.0), 64.0, 0.5);
    let delta_diagonal = compute_normalized_movement_delta(Vec2::new(1.0, 1.0), 64.0, 0.5);
    assert!((delta_cardinal.length() - 32.0).abs() < 1e-5);
    assert!((delta_diagonal.length() - 32.0).abs() < 1e-5);
}

#[test]
fn test_aabb_tile_collision_solid() {
    let grid = WorldGridConfig::default();
    let mut map = FixedCollisionGrid::new_walkable(8, 8);
    map.set_collision(TilePos(2, 2), TileCollision::Solid);

    let hitbox = PlayerHitbox::new(Vec2::ZERO, Vec2::new(8.0, 8.0));
    assert!(check_aabb_collision(
        hitbox.aabb_at(WorldPos(30.0, 30.0)),
        &map,
        &grid
    ));
}

#[test]
fn test_aabb_sliding_collision() {
    let grid = WorldGridConfig::default();
    let mut map = FixedCollisionGrid::new_walkable(8, 8);
    map.set_collision(TilePos(1, 0), TileCollision::Solid);
    map.set_collision(TilePos(1, 1), TileCollision::Solid);

    let hitbox = PlayerHitbox::new(Vec2::ZERO, Vec2::new(8.0, 8.0));
    let (end_pos, applied) = resolve_subtile_movement(
        WorldPos(8.0, 2.0),
        &hitbox,
        Vec2::new(4.0, 5.0),
        &map,
        &grid,
    );
    assert_eq!(applied.x, 0.0);
    assert_eq!(applied.y, 5.0);
    assert_eq!(end_pos, WorldPos(8.0, 7.0));
}

#[test]
fn test_bsp_dungeon_connectivity() {
    let cfg = BspDungeonConfig::default();
    let dungeon = generate_bsp_dungeon("dungeon.integration_bsp", 777_777, &cfg).unwrap();
    assert!(dungeon.is_connected_spawn_to_exit());
}

#[test]
fn test_prefab_deserialization_success() {
    let instance = PrefabLoader::load_from_file("assets/prefabs/starter_house.ron", None).unwrap();
    assert_eq!(instance.instance_id, "prefab.starter_house");
    assert!(instance.is_connected_spawn_to_exit());
}

#[test]
fn test_prefab_deserialization_invalid_format() {
    let invalid = PrefabLoader::load_from_str("not valid ron", DataFormat::Ron, None);
    assert!(matches!(invalid, Err(PrefabError::RonParse(_))));
}
