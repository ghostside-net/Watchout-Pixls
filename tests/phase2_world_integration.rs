//! Integration tests for Phase 2 (`Core World System`) of `watchout_pixls`.
//!
//! Verifies:
//! 1. Tile (`16x16` px) and Chunk (`16x16` tiles) coordinate system & camera tracking.
//! 2. Deterministic seed-based procedural generation (Simplex/Perlin noise for biomes, paths, walls, resources).
//! 3. Multithreaded chunk streaming via `bevy::tasks::AsyncComputeTaskPool` (loading active chunks, unloading distant chunks).
//! 4. Persistence of modified chunks and explored areas (`Binary`, `CompressedRon`, `CompressedJson`) and
//!    exact state restoration when re-entering unloaded chunks vs. deterministic seed reproduction for untouched chunks.

use bevy::prelude::*;
use watchout_pixls::build_watchout_app;
use watchout_pixls::state::{GameState, StateTransitionAction};
use watchout_pixls::window_render::PixelPerfectCamera2d;
use watchout_pixls::world::{
    ChunkCoord, ChunkEntity, ChunkSource, ChunkStreamingConfig, HeroMoveEvent, LocalTileCoord,
    ResourceKind, SaveFormat, TerrainTileKind, TileCoord, WallKind, WorldGridConfig, WorldMap,
    WorldPersistence, WorldSeed, WorldStreamingMetrics,
};

#[test]
fn integration_phase2_chunk_streaming_and_unloading_lifecycle() {
    let mut app = build_watchout_app();

    // Start in MainMenu -> transition to InGame
    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::InGame
    );

    // Default load_radius = 1 around chunk (0, 0) -> 3x3 = 9 active chunks
    {
        let world_map = app.world().resource::<WorldMap>();
        let metrics = app.world().resource::<WorldStreamingMetrics>();
        let persistence = app.world().resource::<WorldPersistence>();

        assert_eq!(world_map.last_center_chunk, Some(ChunkCoord::new(0, 0)));
        assert_eq!(world_map.active_chunk_count(), 9);
        assert_eq!(metrics.active_chunk_count, 9);
        assert!(metrics.total_async_tasks_spawned >= 9);
        assert_eq!(persistence.explored_chunk_count(), 9);

        for dy in -1..=1 {
            for dx in -1..=1 {
                assert!(world_map.is_chunk_loaded(ChunkCoord::new(dx, dy)));
            }
        }
    }

    // Verify ECS ChunkEntity components match active chunks
    let chunk_entity_count = app
        .world_mut()
        .query::<&ChunkEntity>()
        .iter(app.world())
        .count();
    assert_eq!(chunk_entity_count, 9);

    // Move Hero by 1 chunk East (to tile (20, 7) in chunk (1, 0)):
    // Because unload_radius = 2, chunks at x = -1 have distance 2 from chunk (1, 0) and stay loaded (hysteresis!),
    // while new chunks at x = 2 are streamed in (total 4x3 = 12 active chunks).
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToTile(TileCoord::new(20, 7)));
    app.update();

    {
        let world_map = app.world().resource::<WorldMap>();
        assert_eq!(world_map.last_center_chunk, Some(ChunkCoord::new(1, 0)));
        assert_eq!(world_map.active_chunk_count(), 12);
        assert!(world_map.is_chunk_loaded(ChunkCoord::new(-1, 0))); // kept by unload_radius=2 hysteresis
        assert!(world_map.is_chunk_loaded(ChunkCoord::new(2, 0))); // newly streamed in
    }

    // Now teleport Hero far away to tile (160, 160) -> chunk (10, 10)
    // All previous chunks around (0, 0) and (1, 0) exceed unload_radius = 2 and must be unloaded!
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToTile(TileCoord::new(160, 160)));
    app.update();

    {
        let world_map = app.world().resource::<WorldMap>();
        let metrics = app.world().resource::<WorldStreamingMetrics>();
        let persistence = app.world().resource::<WorldPersistence>();

        assert_eq!(world_map.last_center_chunk, Some(ChunkCoord::new(10, 10)));
        assert_eq!(world_map.active_chunk_count(), 9);
        assert!(!world_map.is_chunk_loaded(ChunkCoord::new(0, 0)));
        assert!(!world_map.is_chunk_loaded(ChunkCoord::new(1, 0)));
        assert!(world_map.is_chunk_loaded(ChunkCoord::new(10, 10)));
        assert_eq!(metrics.total_chunks_unloaded, 12);

        // Since no chunks were modified, modified_chunk_count must be 0, while explored_chunk_count is 12 + 9 = 21
        assert_eq!(persistence.modified_chunk_count(), 0);
        assert_eq!(persistence.explored_chunk_count(), 21);
    }

    // Verify camera followed the Hero to chunk (10, 10)
    let camera = app
        .world_mut()
        .query::<&PixelPerfectCamera2d>()
        .single(app.world());
    assert!(camera.position.x > 2000.0 && camera.position.y > 2000.0);
}

#[test]
fn integration_phase2_modified_chunk_persistence_and_reentry_restoration() {
    let mut app = build_watchout_app();

    // Enter InGame
    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    let grid = *app.world().resource::<WorldGridConfig>();
    let seed = *app.world().resource::<WorldSeed>();

    // Snapshot untouched chunk (1, 0) before leaving so we can verify deterministic reproduction later
    let untouched_chunk_snapshot = app
        .world()
        .resource::<WorldMap>()
        .get_chunk(ChunkCoord::new(1, 0))
        .unwrap()
        .clone();
    assert!(!untouched_chunk_snapshot.is_modified);

    // Modify chunk (0, 0) at global tiles (2, 2) and (3, 3), and place/harvest a resource at (4, 4)
    {
        let mut world_map = app.world_mut().resource_mut::<WorldMap>();
        assert!(world_map.set_wall(TileCoord::new(2, 2), &grid, Some(WallKind::AncientPillar)));
        assert!(world_map.set_terrain(TileCoord::new(3, 3), &grid, TerrainTileKind::DirtPath));

        // Put a crystal cluster with yield 5 at (4, 4) and harvest 2 units -> 3 remaining
        let chunk0 = world_map.get_chunk_mut(ChunkCoord::new(0, 0)).unwrap();
        let mut cell44 = *chunk0.get_tile(LocalTileCoord::new(4, 4)).unwrap();
        cell44.wall = None;
        cell44.resource = Some(watchout_pixls::world::ResourceNode::new(
            ResourceKind::CrystalCluster,
            5,
        ));
        chunk0.set_tile(LocalTileCoord::new(4, 4), cell44);

        let harvested = world_map.harvest_resource(TileCoord::new(4, 4), &grid, 2);
        assert_eq!(harvested, Some((ResourceKind::CrystalCluster, 2)));
        assert!(
            world_map
                .get_chunk(ChunkCoord::new(0, 0))
                .unwrap()
                .is_modified
        );
    }

    // Teleport Hero far away to chunk (-8, -8) (tile (-120, -120)) so chunk (0, 0) and (1, 0) unload
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToTile(TileCoord::new(-120, -120)));
    app.update();

    {
        let world_map = app.world().resource::<WorldMap>();
        let persistence = app.world().resource::<WorldPersistence>();
        let metrics = app.world().resource::<WorldStreamingMetrics>();

        assert!(!world_map.is_chunk_loaded(ChunkCoord::new(0, 0)));
        assert!(!world_map.is_chunk_loaded(ChunkCoord::new(1, 0)));

        // Only chunk (0, 0) was modified, so only 1 chunk is stored in modified_chunks!
        assert_eq!(persistence.modified_chunk_count(), 1);
        assert!(persistence.has_modified_chunk(ChunkCoord::new(0, 0)));
        assert!(!persistence.has_modified_chunk(ChunkCoord::new(1, 0)));
        assert_eq!(metrics.total_modified_chunks_saved, 1);
    }

    // Now teleport Hero back to origin tile (9, 7) in chunk (0, 0)!
    app.world_mut()
        .send_event(HeroMoveEvent::TeleportToTile(TileCoord::new(9, 7)));
    app.update();

    {
        let world_map = app.world().resource::<WorldMap>();
        let metrics = app.world().resource::<WorldStreamingMetrics>();

        assert!(world_map.is_chunk_loaded(ChunkCoord::new(0, 0)));
        assert!(world_map.is_chunk_loaded(ChunkCoord::new(1, 0)));
        assert!(metrics.total_chunks_restored_from_save >= 1);

        // Verify modified chunk (0, 0) has all player changes intact
        let restored_cell_22 = world_map.get_tile(TileCoord::new(2, 2), &grid).unwrap();
        assert_eq!(restored_cell_22.wall, Some(WallKind::AncientPillar));
        assert!(!restored_cell_22.is_walkable());

        let restored_cell_33 = world_map.get_tile(TileCoord::new(3, 3), &grid).unwrap();
        assert_eq!(restored_cell_33.terrain, TerrainTileKind::DirtPath);

        let restored_cell_44 = world_map.get_tile(TileCoord::new(4, 4), &grid).unwrap();
        assert_eq!(
            restored_cell_44.resource,
            Some(watchout_pixls::world::ResourceNode::new(
                ResourceKind::CrystalCluster,
                3
            ))
        );

        // Verify untouched chunk (1, 0) was deterministically reproduced from seed
        let reproduced_chunk_10 = world_map.get_chunk(ChunkCoord::new(1, 0)).unwrap();
        assert!(!reproduced_chunk_10.is_modified);
        assert_eq!(reproduced_chunk_10.tiles, untouched_chunk_snapshot.tiles);
    }

    // VerifyChunkEntity source tags in ECS
    for chunk_entity in app.world_mut().query::<&ChunkEntity>().iter(app.world()) {
        if chunk_entity.coord == ChunkCoord::new(0, 0) {
            assert_eq!(chunk_entity.source, ChunkSource::RestoredModified);
        } else {
            assert_eq!(chunk_entity.source, ChunkSource::GeneratedFromSeed);
        }
    }

    // Save world state to disk in Binary (.wpx) and CompressedRon (.cron) formats and reload into a new App
    let tmp_bin = std::env::temp_dir().join("watchout_pixls_phase2_world.wpx");
    let tmp_cron = std::env::temp_dir().join("watchout_pixls_phase2_world.cron");

    {
        let world_map = app.world().resource::<WorldMap>();
        let persistence = app.world().resource::<WorldPersistence>();

        let bin_size = persistence
            .save_to_file(
                &tmp_bin,
                seed,
                grid,
                Some(world_map.active_chunks()),
                SaveFormat::from_path(&tmp_bin).unwrap(),
            )
            .unwrap();
        assert!(bin_size > 16);

        let cron_size = persistence
            .save_to_file(
                &tmp_cron,
                seed,
                grid,
                Some(world_map.active_chunks()),
                SaveFormat::from_path(&tmp_cron).unwrap(),
            )
            .unwrap();
        assert!(cron_size > 10);
    }

    // Load Binary save file into a fresh WorldPersistence and verify state
    let loaded_save = WorldPersistence::load_from_file(&tmp_bin, SaveFormat::Binary).unwrap();
    assert_eq!(loaded_save.seed, seed);
    assert_eq!(loaded_save.modified_chunks.len(), 1);
    assert_eq!(loaded_save.modified_chunks[0].coord, ChunkCoord::new(0, 0));
    assert_eq!(loaded_save.explored_chunks.len(), 18); // 9 around (0,0) + 9 around (-8,-8)

    // Also verify CompressedRon file loads identically
    let loaded_cron =
        WorldPersistence::load_from_file(&tmp_cron, SaveFormat::CompressedRon).unwrap();
    assert_eq!(loaded_cron, loaded_save);

    let _ = std::fs::remove_file(&tmp_bin);
    let _ = std::fs::remove_file(&tmp_cron);
}

#[test]
fn integration_phase2_non_blocking_async_task_polling() {
    let mut app = build_watchout_app();

    // Configure streaming to allow multi-tick non-blocking polling (complete_within_frame = false)
    app.insert_resource(ChunkStreamingConfig {
        load_radius: 1,
        unload_radius: 2,
        max_tasks_per_frame: 4, // throttle to 4 tasks per tick to verify batched streaming
        complete_within_frame: false,
    });

    app.update();
    app.world_mut().send_event(StateTransitionAction::StartGame);

    // Run enough ticks + brief thread yields so background worker threads finish all 9 chunks in 3 batches (4 + 4 + 1)
    for _ in 0..8 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let world_map = app.world().resource::<WorldMap>();
    assert_eq!(world_map.active_chunk_count(), 9);
    assert_eq!(world_map.in_flight_task_count(), 0);
}
