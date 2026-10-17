//! `watchout_pixls` — Modular 2D Pixel-Art Game Core built on Bevy ECS.
//!
//! # Architecture Overview (Phase 1)
//! - [`window_render`]: Fixed retro resolution (`320x240` / `480x270`), integer scaling (`3x`/`4x`),
//!   non-resizable/non-fullscreen window rules, and [`window_render::PixelPerfectCamera2d`] with
//!   Nearest-Neighbor point sampling.
//! - [`palette_sprite`]: 4-to-16 color [`palette_sprite::PastelGameBoyPalette`] and indexed
//!   pixel-art lookup for 2D tiles and sprites.
//! - [`content`]: [`content::ContentDefinition`] trait and generic [`content::ContentRegistry`]
//!   for Items, Classes, Entities, and Tiles loaded via RON or JSON.
//! - [`modding`]: Plugin-based [`modding::ModExtension`] trait, declarative [`modding::ModManifest`],
//!   and optional runtime shared-library loading (`libloading`).
//! - [`state`]: Bevy `States`-driven game loop (`MainMenu`, `InGame`, `Paused`).
//! - [`world`]: Core World System (Phase 2) with `16x16` tile & chunk coordinates,
//!   deterministic Simplex/Perlin procedural generation, multithreaded chunk streaming
//!   (`bevy::tasks`), and binary/compressed persistence.

pub mod content;
#[cfg(feature = "desktop-window")]
pub mod desktop;
pub mod interaction;
pub mod modding;
pub mod palette_sprite;
pub mod player;
pub mod scene;
pub mod state;
pub mod window_render;
pub mod world;

pub use interaction::{
    can_trigger_interaction, Interactable, InteractableKind, InteractableTrigger,
    InteractionOutcomeEvent, Phase3GameplayPlugin, PlayerInteractEvent, TriggerMode,
};
pub use player::{
    check_aabb_collision, compute_normalized_movement_delta, compute_y_sort_order,
    overlapped_tiles_for_aabb, resolve_subtile_movement, Aabb, CollisionMap, FacingDirection,
    FixedCollisionGrid, PlayerHitbox, PlayerInputState, PlayerMovement, PlayerState, YSort,
};
pub use scene::{
    generate_bsp_dungeon, BspDungeonConfig, BspRoom, DungeonGenError, DungeonState, InstanceProp,
    InstancePropKind, InstanceSource, InstanceTile, OverworldState, PrefabDefinition, PrefabError,
    PrefabLoader, Scene, SceneManager, SceneTransitionEvent,
};
pub use world::{TileCollision, TilePos, WorldPos};

use std::path::Path;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use crate::content::ContentPlugin;
use crate::modding::ModLoaderPlugin;
use crate::palette_sprite::{PaletteAndSpritePlugin, PastelGameBoyPalette};
use crate::state::GameStatePlugin;
use crate::window_render::{RetroWindowConfig, WindowAndRenderPlugin};
use crate::world::WorldPlugin;

/// Modular Bevy `PluginGroup` bundling all Phase 1, Phase 2 & Phase 3 core plugins for `watchout_pixls`.
#[derive(Default)]
pub struct WatchoutPixlsPlugins {
    pub window_config: RetroWindowConfig,
    pub palette: PastelGameBoyPalette,
    pub world_plugin: WorldPlugin,
}

impl WatchoutPixlsPlugins {
    #[must_use]
    pub fn new(window_config: RetroWindowConfig, palette: PastelGameBoyPalette) -> Self {
        Self {
            window_config,
            palette,
            world_plugin: WorldPlugin::default(),
        }
    }

    #[must_use]
    pub fn with_world_plugin(mut self, world_plugin: WorldPlugin) -> Self {
        self.world_plugin = world_plugin;
        self
    }
}

impl PluginGroup for WatchoutPixlsPlugins {
    fn build(self) -> PluginGroupBuilder {
        let mut group = PluginGroupBuilder::start::<Self>()
            .add(WindowAndRenderPlugin::new(self.window_config))
            .add(PaletteAndSpritePlugin::new(self.palette))
            .add(ContentPlugin)
            .add(GameStatePlugin)
            .add(self.world_plugin)
            .add(Phase3GameplayPlugin);

        let mut content_pack_dir: Option<std::path::PathBuf> = None;
        let cwd_pack = Path::new("assets/content");
        if cwd_pack.exists() {
            content_pack_dir = Some(cwd_pack.to_path_buf());
        } else if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let exe_relative = exe_dir.join("assets/content");
                let target_relative = exe_dir.join("../../assets/content");
                if exe_relative.exists() {
                    content_pack_dir = Some(exe_relative);
                } else if target_relative.exists() {
                    content_pack_dir = Some(target_relative);
                }
            }
        }

        if let Some(pack) = content_pack_dir {
            group = group.add(ModLoaderPlugin::new().with_content_pack_dir(pack));
        } else {
            group = group.add(ModLoaderPlugin::new());
        }

        group
    }
}

/// Builds a ready-to-run Bevy `App` configured with `MinimalPlugins` and [`WatchoutPixlsPlugins`].
#[must_use]
pub fn build_watchout_app() -> App {
    build_watchout_app_with_config(
        RetroWindowConfig::default(),
        PastelGameBoyPalette::default(),
    )
}

/// Builds a Bevy `App` with custom window and palette configurations.
#[must_use]
pub fn build_watchout_app_with_config(
    window_config: RetroWindowConfig,
    palette: PastelGameBoyPalette,
) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(WatchoutPixlsPlugins::new(window_config, palette));
    app
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::content::{
        ClassDefinition, ContentDefinition, DataFormat, EntityArchetype, GameContentRegistries,
        ItemCategory, ItemDefinition, TileDefinition,
    };
    use crate::modding::{ModError, ModExtension, ModManager, ModManifest, ModMetadata};
    use crate::palette_sprite::{IndexedPixelGrid, PaletteError, RgbaColor};
    use crate::state::{
        GameLoopMetrics, GameState, HeroController, InGameEntity, MainMenuEntity,
        StateTransitionAction,
    };
    use crate::window_render::{
        IntegerScaling, PixelCanvas, PixelPerfectCamera2d, RenderError, RetroResolutionPreset,
        TextureFilterMode,
    };

    #[test]
    fn test_window_config_enforces_fixed_integer_scaling_and_nearest_filter() {
        let cfg = RetroWindowConfig::default();
        assert_eq!(cfg.native_width(), 320);
        assert_eq!(cfg.native_height(), 240);
        assert_eq!(cfg.scaling, IntegerScaling::Scale3x);
        assert_eq!(cfg.physical_width(), 960);
        assert_eq!(cfg.physical_height(), 720);
        assert_eq!(cfg.filter_mode, TextureFilterMode::Nearest);
        assert!(!cfg.resizable);
        assert!(!cfg.fullscreen);
        assert!(cfg.validate().is_ok());

        let widescreen = RetroWindowConfig {
            resolution: RetroResolutionPreset::Widescreen480x270,
            scaling: IntegerScaling::Scale4x,
            ..RetroWindowConfig::default()
        };
        assert_eq!(widescreen.physical_width(), 1920);
        assert_eq!(widescreen.physical_height(), 1080);
        assert!(widescreen.validate().is_ok());

        // Reject linear blur filter
        let blurred = RetroWindowConfig {
            filter_mode: TextureFilterMode::Linear,
            ..RetroWindowConfig::default()
        };
        assert!(matches!(
            blurred.validate(),
            Err(RenderError::BlurFilterForbidden)
        ));

        // Reject free resizing
        let resizable = RetroWindowConfig {
            resizable: true,
            ..RetroWindowConfig::default()
        };
        assert!(matches!(
            resizable.validate(),
            Err(RenderError::ResizingForbidden)
        ));

        // Reject fullscreen
        let fullscreen = RetroWindowConfig {
            fullscreen: true,
            ..RetroWindowConfig::default()
        };
        assert!(matches!(
            fullscreen.validate(),
            Err(RenderError::FullscreenForbidden)
        ));
    }

    #[test]
    fn test_pixel_perfect_camera_snaps_subpixel_coordinates() {
        let mut cam = PixelPerfectCamera2d::new(320, 240);
        cam.position = Vec2::new(10.4, 20.6);
        assert_eq!(cam.snapped_origin(), IVec2::new(10, 21));

        let entity_pos = Vec2::new(50.2, 100.8);
        let screen_px = cam.world_to_screen_pixel(entity_pos);
        // 50 - 10 = 40, 101 - 21 = 80
        assert_eq!(screen_px, IVec2::new(40, 80));
    }

    #[test]
    fn test_pastel_gameboy_palettes_and_pixel_lookup() {
        let dmg4 = PastelGameBoyPalette::pastel_dmg_4();
        assert_eq!(dmg4.len(), 4);

        let dmg8 = PastelGameBoyPalette::pastel_dmg_8();
        assert_eq!(dmg8.len(), 8);
        assert_eq!(dmg8.lookup(0).unwrap(), RgbaColor::from_hex(0xFAF3E0));
        assert_eq!(dmg8.lookup(3).unwrap(), RgbaColor::from_hex(0x4A5859));
        assert_eq!(dmg8.lookup(4).unwrap(), RgbaColor::from_hex(0xF7C5CC));

        let deluxe16 = PastelGameBoyPalette::pastel_deluxe_16();
        assert_eq!(deluxe16.len(), 16);
        assert!(deluxe16.lookup(16).is_err());

        // Palette with fewer than 4 or more than 16 colors is rejected
        assert_eq!(
            PastelGameBoyPalette::new("TooSmall", vec![RgbaColor::BLACK; 3]),
            Err(PaletteError::InvalidPaletteSize(3))
        );
        assert_eq!(
            PastelGameBoyPalette::new("TooLarge", vec![RgbaColor::BLACK; 17]),
            Err(PaletteError::InvalidPaletteSize(17))
        );

        // Verify sprite transparency vs tile opacity for index 0
        let hero = IndexedPixelGrid::hero_pixl_sprite_8x8();
        assert_eq!(hero.sample_color(0, 0, &dmg8).unwrap(), None); // index 0 is transparent on sprites
        assert_eq!(
            hero.sample_color(2, 0, &dmg8).unwrap(),
            Some(RgbaColor::from_hex(0xF9E79F)) // index 6 = Buttercup Sun crown
        );

        let meadow = IndexedPixelGrid::pastel_meadow_tile_8x8();
        assert_eq!(
            meadow.sample_color(1, 1, &dmg8).unwrap(),
            Some(RgbaColor::from_hex(0xFAF3E0)) // index 0 is opaque Cream Mist on tiles
        );
    }

    #[test]
    fn test_nearest_neighbor_integer_upscaling_is_exact() {
        let cfg = RetroWindowConfig::default(); // 320x240 @ 3x -> 960x720
        let palette = PastelGameBoyPalette::pastel_dmg_8();
        let mut canvas = PixelCanvas::from_config(&cfg);

        let c_mint = palette.lookup(1).unwrap();
        let c_peach = palette.lookup(4).unwrap();
        canvas.clear(c_mint);
        canvas.set_pixel(5, 7, c_peach);
        canvas.upscale_nearest_neighbor();

        // Native pixel (5, 7) must map to the exact 3x3 physical block [15..18, 21..24]
        let expected_u32 = c_peach.to_softbuffer_u32();
        for dy in 0..3 {
            for dx in 0..3 {
                assert_eq!(
                    canvas.get_scaled_pixel_u32(5 * 3 + dx, 7 * 3 + dy),
                    Some(expected_u32)
                );
            }
        }
        // Adjacent physical pixel (14, 21) must remain c_mint with zero blending
        assert_eq!(
            canvas.get_scaled_pixel_u32(14, 21),
            Some(c_mint.to_softbuffer_u32())
        );
    }

    #[test]
    fn test_game_loop_states_and_render_verification() {
        let mut app = build_watchout_app();

        // First update runs Startup + OnEnter(GameState::MainMenu) + PostUpdate (render)
        app.update();

        assert_eq!(
            *app.world().resource::<State<GameState>>().get(),
            GameState::MainMenu
        );
        assert_eq!(app.world().resource::<GameLoopMetrics>().main_menu_ticks, 1);

        let menu_entities = app
            .world_mut()
            .query::<&MainMenuEntity>()
            .iter(app.world())
            .count();
        assert!(menu_entities >= 2);

        // Transition to InGame state
        app.world_mut().send_event(StateTransitionAction::StartGame);
        app.update(); // processes event and queues NextState::InGame
        app.update(); // runs OnExit(MainMenu), OnEnter(InGame), and renders the InGame scene

        assert_eq!(
            *app.world().resource::<State<GameState>>().get(),
            GameState::InGame
        );
        assert!(app.world().resource::<GameLoopMetrics>().in_game_ticks >= 1);

        // MainMenu entities must be despawned, InGame entities + HeroController must exist
        let menu_entities_after = app
            .world_mut()
            .query::<&MainMenuEntity>()
            .iter(app.world())
            .count();
        assert_eq!(menu_entities_after, 0);

        let hero_count = app
            .world_mut()
            .query::<(&InGameEntity, &HeroController)>()
            .iter(app.world())
            .count();
        assert_eq!(hero_count, 1);

        // Verify the rendered PixelCanvas has the Pastel GameBoy palette swatches and Hero sprite pixels
        let palette = app.world().resource::<PastelGameBoyPalette>().clone();
        let canvas = app.world().resource::<PixelCanvas>();
        assert!(canvas.frame_count >= 2);

        // Swatch strip in InGame starts at (8, 8) with 8x8 blocks for each palette color
        for i in 0..palette.len() {
            let swatch_x = 8 + (i as u32) * 8 + 2;
            let swatch_y = 8 + 2;
            assert_eq!(
                canvas.get_pixel(swatch_x, swatch_y),
                Some(palette.lookup(i as u8).unwrap())
            );
        }

        // Hero sprite is at world (152, 112); pixel (2, 0) of hero sprite has index 6 (Buttercup Sun)
        assert_eq!(
            canvas.get_pixel(152 + 2, 112),
            Some(palette.lookup(6).unwrap())
        );
        // Hero tunic at local (2, 4) has index 4 (Peach Blossom)
        assert_eq!(
            canvas.get_pixel(152 + 2, 112 + 4),
            Some(palette.lookup(4).unwrap())
        );

        // Pause and return to MainMenu
        app.world_mut()
            .send_event(StateTransitionAction::TogglePause);
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<GameState>>().get(),
            GameState::Paused
        );

        app.world_mut()
            .send_event(StateTransitionAction::ReturnToMainMenu);
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<GameState>>().get(),
            GameState::MainMenu
        );
    }

    struct TestCustomMod;

    impl ModExtension for TestCustomMod {
        fn metadata(&self) -> ModMetadata {
            ModMetadata {
                id: "mod.unit_test_expansion".to_string(),
                name: "Unit Test Expansion".to_string(),
                version: "1.0.0".to_string(),
                author: "TestSuite".to_string(),
                description: "Verifies trait-based mod registration".to_string(),
            }
        }

        fn register_content(&self, registries: &mut GameContentRegistries) -> Result<(), ModError> {
            registries
                .items
                .register_or_override(ItemDefinition {
                    id: "item.mod_grappling_hook".to_string(),
                    display_name: "Pastel Grappling Hook".to_string(),
                    category: ItemCategory::Tool,
                    max_stack: 1,
                    value: 500,
                    power_bonus: 5,
                    sprite_id: "sprite_grappling_hook".to_string(),
                })
                .map_err(|source| ModError::Content {
                    mod_id: self.metadata().id,
                    source,
                })?;
            Ok(())
        }
    }

    #[test]
    fn test_content_definitions_ron_json_and_mod_extensions() {
        let mut registries = GameContentRegistries::new();
        registries.register_core_defaults().unwrap();

        // 1. Test RON loading for ItemDefinition & ClassDefinition
        let ron_item = r#"
            (
                id: "item.star_shield",
                display_name: "Starlight Buckler",
                category: Armor,
                max_stack: 1,
                value: 180,
                power_bonus: 12,
                sprite_id: "sprite_star_shield",
            )
        "#;
        registries
            .items
            .load_single_from_str(ron_item, DataFormat::Ron)
            .unwrap();
        assert_eq!(
            registries
                .items
                .get("item.star_shield")
                .unwrap()
                .display_name(),
            "Starlight Buckler"
        );

        // 2. Test JSON loading for EntityDefinition & TileDefinition
        let json_entity = r#"
            {
                "id": "entity.crystal_golem",
                "display_name": "Crystal Golem",
                "archetype": "Monster",
                "max_health": 250,
                "move_speed": 20.0,
                "hostile": true,
                "sprite_id": "sprite_crystal_golem",
                "drop_table": ["item.star_shield"]
            }
        "#;
        registries
            .entities
            .load_single_from_str(json_entity, DataFormat::Json)
            .unwrap();
        let golem = registries.entities.get("entity.crystal_golem").unwrap();
        assert_eq!(golem.archetype, EntityArchetype::Monster);
        assert_eq!(golem.max_health, 250);

        // 3. Test ModExtension trait registration
        let mut mod_manager = ModManager::new();
        mod_manager
            .register_extension(Arc::new(TestCustomMod), &mut registries)
            .unwrap();
        assert!(mod_manager.is_loaded("mod.unit_test_expansion"));
        assert!(registries.items.get("item.mod_grappling_hook").is_some());

        // 4. Test loading external mod pack from assets/content/mod.ron
        let manifest_path = Path::new("assets/content/mod.ron");
        if manifest_path.exists() {
            let raw = std::fs::read_to_string(manifest_path).unwrap();
            let manifest: ModManifest = ron::from_str(&raw).unwrap();
            mod_manager
                .load_manifest_from_dir(&manifest, "assets/content", &mut registries)
                .unwrap();

            assert!(mod_manager.is_loaded("mod.pastel_frontier"));
            assert!(registries.items.get("item.lavender_wand").is_some());
            assert!(registries.classes.get("class.pastel_alchemist").is_some());
            assert!(registries.entities.get("entity.peach_wisp").is_some());
            assert!(registries.tiles.get("tile.sky_stream").is_some());
        }

        // 5. Verify validation rejects invalid definitions
        let invalid_tile = TileDefinition {
            id: "tile.broken".to_string(),
            display_name: "Broken Tile".to_string(),
            walkable: true,
            transparent: true,
            movement_cost: 0.0, // invalid: must be > 0.0
            sprite_id: "broken".to_string(),
            custom_indices_8x8: None,
        };
        assert!(invalid_tile.validate().is_err());

        let invalid_class = ClassDefinition {
            id: "".to_string(),
            display_name: "Unnamed".to_string(),
            base_health: 10,
            base_mana: 10,
            base_speed: 10.0,
            starting_items: vec![],
            passive_trait: "None".to_string(),
        };
        assert!(invalid_class.validate().is_err());
    }

    #[test]
    fn test_phase2_coordinates_and_euclidean_chunk_grid() {
        use crate::world::{
            ChunkCoord, GridError, LocalTileCoord, TileCoord, WorldGridConfig,
            DEFAULT_CHUNK_SIZE_TILES, DEFAULT_TILE_SIZE_PX,
        };

        let grid = WorldGridConfig::default();
        assert_eq!(grid.tile_size_px, DEFAULT_TILE_SIZE_PX);
        assert_eq!(grid.chunk_size_tiles, DEFAULT_CHUNK_SIZE_TILES);
        assert_eq!(grid.tiles_per_chunk(), 256);
        assert_eq!(grid.chunk_size_px(), 256);

        assert_eq!(
            WorldGridConfig::new(0, 16),
            Err(GridError::InvalidTileSize(0))
        );
        assert_eq!(
            WorldGridConfig::new(16, 0),
            Err(GridError::InvalidChunkSize(0))
        );

        // Positive and boundary pixel-to-tile conversions
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(0.0, 0.0)),
            TileCoord::new(0, 0)
        );
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(15.9, 15.9)),
            TileCoord::new(0, 0)
        );
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(16.0, 32.0)),
            TileCoord::new(1, 2)
        );

        // Negative pixel-to-tile conversions (Euclidean floor)
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(-0.1, -0.1)),
            TileCoord::new(-1, -1)
        );
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(-16.0, -16.0)),
            TileCoord::new(-1, -1)
        );
        assert_eq!(
            grid.world_pixel_to_tile(Vec2::new(-16.1, -32.1)),
            TileCoord::new(-2, -3)
        );

        // Tile <-> (Chunk, Local) conversions across negative and positive quadrants
        for ty in -35..=35 {
            for tx in -35..=35 {
                let tile = TileCoord::new(tx, ty);
                let (chunk, local) = grid.tile_to_chunk_and_local(tile);
                assert!(local.x < 16 && local.y < 16);
                let reconstructed = grid.chunk_and_local_to_tile(chunk, local);
                assert_eq!(reconstructed, tile);

                let idx = grid.local_to_index(local).unwrap();
                assert_eq!(grid.index_to_local(idx), Some(local));
            }
        }

        // Specific negative chunk boundary verification
        assert_eq!(
            grid.tile_to_chunk_and_local(TileCoord::new(-1, -1)),
            (ChunkCoord::new(-1, -1), LocalTileCoord::new(15, 15))
        );
        assert_eq!(
            grid.tile_to_chunk_and_local(TileCoord::new(-16, -16)),
            (ChunkCoord::new(-1, -1), LocalTileCoord::new(0, 0))
        );
        assert_eq!(
            grid.tile_to_chunk_and_local(TileCoord::new(-17, 0)),
            (ChunkCoord::new(-2, 0), LocalTileCoord::new(15, 0))
        );
    }

    #[test]
    fn test_phase2_deterministic_world_generation_and_biomes() {
        use crate::world::{
            ChunkCoord, LocalTileCoord, WorldGenerator, WorldGridConfig, WorldSeed,
        };
        use std::collections::BTreeSet;

        let grid = WorldGridConfig::default();
        let seed_a = WorldSeed::new(42_424_242);
        let seed_b = WorldSeed::new(99_999_999);

        let gen_a1 = WorldGenerator::new(seed_a);
        let gen_a2 = WorldGenerator::new(seed_a);
        let gen_b = WorldGenerator::new(seed_b);

        // Same seed must always produce identical chunks across independent generators
        for cy in -2..=2 {
            for cx in -2..=2 {
                let coord = ChunkCoord::new(cx, cy);
                let c1 = gen_a1.generate_chunk(coord, &grid);
                let c2 = gen_a2.generate_chunk(coord, &grid);
                assert_eq!(c1, c2);
                assert!(!c1.is_modified);
                assert_eq!(c1.tiles.len(), 256);
            }
        }

        // Different seeds must produce different worlds
        let diff_a = gen_a1.generate_chunk(ChunkCoord::new(2, -1), &grid);
        let diff_b = gen_b.generate_chunk(ChunkCoord::new(2, -1), &grid);
        assert_ne!(diff_a.tiles, diff_b.tiles);

        // Survey a region of chunks to verify all 5 biomes, paths, walls, and resources appear
        let mut biomes = BTreeSet::new();
        let mut resources = BTreeSet::new();
        let mut total_paths = 0usize;
        let mut total_walls = 0usize;

        for cy in -6..=6 {
            for cx in -6..=6 {
                let chunk = gen_a1.generate_chunk(ChunkCoord::new(cx, cy), &grid);
                total_paths += chunk.path_count();
                total_walls += chunk.wall_count();
                for cell in &chunk.tiles {
                    biomes.insert(cell.biome);
                    if let Some(res) = cell.resource {
                        resources.insert(res.kind);
                    }
                    if cell.is_path() {
                        assert!(
                            cell.is_walkable(),
                            "Generated paths must always be walkable"
                        );
                        assert!(
                            cell.resource.is_none(),
                            "Paths must be clear of resource obstacles"
                        );
                    }
                }
            }
        }

        assert_eq!(
            biomes.len(),
            5,
            "All 5 biomes must be generated across the world"
        );
        assert_eq!(
            resources.len(),
            5,
            "All 5 resource kinds must be generated across the world"
        );
        assert!(
            total_paths > 100,
            "Winding paths must be generated across chunks"
        );
        assert!(
            total_walls > 20,
            "Highland cliffs and walls must be generated"
        );

        // Verify starter sanctuary around (9, 7) in chunk (0, 0) is walkable
        let origin_chunk = gen_a1.generate_chunk(ChunkCoord::new(0, 0), &grid);
        let spawn_cell = origin_chunk.get_tile(LocalTileCoord::new(9, 7)).unwrap();
        assert!(spawn_cell.is_walkable());
    }

    #[test]
    fn test_phase2_persistence_formats_and_delta_reconstruction() {
        use crate::world::{
            ChunkCoord, LocalTileCoord, PersistenceError, SaveFormat, WallKind, WorldGenerator,
            WorldGridConfig, WorldPersistence, WorldSeed,
        };

        let seed = WorldSeed::new(123_456_789);
        let grid = WorldGridConfig::default();
        let generator = WorldGenerator::new(seed);

        let mut untouched = generator.generate_chunk(ChunkCoord::new(0, 0), &grid);
        let mut modified = generator.generate_chunk(ChunkCoord::new(1, -1), &grid);

        // Modify chunk (1, -1): place a wall and harvest/modify terrain
        assert!(modified.set_wall(LocalTileCoord::new(4, 5), Some(WallKind::CobbleWall)));
        assert!(modified.is_modified);
        assert!(!untouched.is_modified);

        let mut persistence = WorldPersistence::new();
        persistence.mark_chunk_explored(&untouched, 10);
        persistence.mark_chunk_explored(&modified, 25);

        // Untouched chunk must NOT be stored in modified_chunks
        assert!(!persistence.store_chunk_if_modified(&untouched));
        assert!(persistence.store_chunk_if_modified(&modified));
        assert_eq!(persistence.explored_chunk_count(), 2);
        assert_eq!(persistence.modified_chunk_count(), 1);

        // Test all 5 serialization formats roundtrip
        for format in [
            SaveFormat::Binary,
            SaveFormat::CompressedRon,
            SaveFormat::CompressedJson,
            SaveFormat::Ron,
            SaveFormat::Json,
        ] {
            let bytes = persistence
                .serialize_bytes(seed, grid, None, format)
                .unwrap();
            let decoded = WorldPersistence::deserialize_bytes(&bytes, format).unwrap();
            assert_eq!(decoded.seed, seed);
            assert_eq!(decoded.grid_config, grid);
            assert_eq!(decoded.explored_chunks.len(), 2);
            assert_eq!(decoded.modified_chunks.len(), 1);
            assert_eq!(decoded.modified_chunks[0], modified);

            // Verify delta calculation and reconstruction
            let deltas = decoded.compute_chunk_deltas(&decoded.modified_chunks[0]);
            assert_eq!(deltas.len(), 1);
            let reconstructed = crate::world::WorldSaveData::reconstruct_chunk_from_deltas(
                seed,
                &grid,
                ChunkCoord::new(1, -1),
                &deltas,
                modified.revision,
            );
            assert_eq!(reconstructed, modified);
        }

        // Verify binary CRC32 integrity check rejects corrupted payload
        let mut binary_bytes = persistence
            .serialize_bytes(seed, grid, None, SaveFormat::Binary)
            .unwrap();
        // Flip a bit in the stored CRC32 header (bytes 12..16)
        binary_bytes[12] ^= 0xFF;
        assert!(matches!(
            WorldPersistence::deserialize_bytes(&binary_bytes, SaveFormat::Binary),
            Err(PersistenceError::ChecksumMismatch { .. })
        ));

        let _ = &mut untouched;
    }

    #[test]
    fn test_player_diagonal_movement_normalization() {
        let speed = 100.0;
        let dt = 0.1; // Expected travel distance = 10.0 px regardless of direction

        let cardinal_right = compute_normalized_movement_delta(Vec2::new(1.0, 0.0), speed, dt);
        let cardinal_up = compute_normalized_movement_delta(Vec2::new(0.0, -1.0), speed, dt);
        let diagonal_ne = compute_normalized_movement_delta(Vec2::new(1.0, -1.0), speed, dt);
        let diagonal_sw = compute_normalized_movement_delta(Vec2::new(-1.0, 1.0), speed, dt);

        assert!((cardinal_right.length() - 10.0).abs() < 1e-5);
        assert!((cardinal_up.length() - 10.0).abs() < 1e-5);

        // Diagonal distance must NOT be sqrt(2) * 10.0 (~14.1421), it must be normalized to 10.0
        assert!((diagonal_ne.length() - 10.0).abs() < 1e-5);
        assert!((diagonal_sw.length() - 10.0).abs() < 1e-5);
        assert!((diagonal_ne.x - 10.0 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((diagonal_ne.y + 10.0 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);

        // Zero input or non-positive dt yields zero movement
        assert_eq!(
            compute_normalized_movement_delta(Vec2::ZERO, speed, dt),
            Vec2::ZERO
        );
        assert_eq!(
            compute_normalized_movement_delta(Vec2::ONE, speed, 0.0),
            Vec2::ZERO
        );
    }

    #[test]
    fn test_aabb_tile_collision_solid() {
        use crate::world::WorldGridConfig;

        let grid = WorldGridConfig::default(); // 16x16 px per tile
        let mut map = FixedCollisionGrid::new_walkable(10, 10);
        // Place a solid wall at tile (1, 0) -> world X in [16.0, 32.0), Y in [0.0, 16.0)
        map.set_collision(TilePos(1, 0), TileCollision::Solid);
        map.set_collision(TilePos(0, 2), TileCollision::Water);

        let hitbox = PlayerHitbox::new(Vec2::new(2.0, 4.0), Vec2::new(8.0, 8.0));

        // 1. AABB entirely inside walkable tile (0, 0) -> no collision
        let safe_pos = WorldPos(2.0, 2.0);
        assert!(!check_aabb_collision(hitbox.aabb_at(safe_pos), &map, &grid));

        // 2. AABB intersects solid tile (1, 0) -> collision detected & movement blocked
        let overlapping_pos = WorldPos(10.0, 2.0); // hitbox X: [12.0, 20.0) overlaps tile (1, 0)
        assert!(check_aabb_collision(
            hitbox.aabb_at(overlapping_pos),
            &map,
            &grid
        ));

        let (resolved_pos, applied_delta) = resolve_subtile_movement(
            WorldPos(6.0, 2.0),
            &hitbox,
            Vec2::new(4.0, 0.0),
            &map,
            &grid,
        );
        assert_eq!(resolved_pos, WorldPos(6.0, 2.0));
        assert_eq!(applied_delta, Vec2::ZERO);

        // 3. Out-of-bounds negative tile (-1, 0) deterministically acts as Solid without panic
        assert_eq!(
            map.tile_collision_at(TilePos(-1, 0), &grid),
            TileCollision::Solid
        );
        assert!(check_aabb_collision(
            hitbox.aabb_at(WorldPos(-4.0, 2.0)),
            &map,
            &grid
        ));
    }

    #[test]
    fn test_aabb_sliding_collision() {
        use crate::world::WorldGridConfig;

        let grid = WorldGridConfig::default(); // 16x16 px per tile
        let mut map = FixedCollisionGrid::new_walkable(10, 10);
        // Vertical wall along column x = 1 (tiles (1, 0), (1, 1), (1, 2))
        map.set_collision(TilePos(1, 0), TileCollision::Solid);
        map.set_collision(TilePos(1, 1), TileCollision::Solid);
        map.set_collision(TilePos(1, 2), TileCollision::Solid);

        // Hitbox of size 8x8 at offset (0, 0); starting at (8.0, 4.0) so right edge is flush at x = 16.0
        let hitbox = PlayerHitbox::new(Vec2::ZERO, Vec2::new(8.0, 8.0));
        let start_pos = WorldPos(8.0, 4.0);

        // Move diagonally into the vertical wall: (+5.0 on X, +6.0 on Y)
        // X movement is blocked by the wall at x = 16.0 (X delta = 0.0), while Y movement slides freely (+6.0)!
        let (end_pos, applied_delta) =
            resolve_subtile_movement(start_pos, &hitbox, Vec2::new(5.0, 6.0), &map, &grid);

        assert!((applied_delta.x - 0.0).abs() < 1e-5);
        assert!((applied_delta.y - 6.0).abs() < 1e-5);
        assert_eq!(end_pos, WorldPos(8.0, 10.0));

        // Verify anti-tunneling: a huge horizontal delta (e.g. +48.0 px across a 16px wall) must NOT tunnel through column x = 1
        let (anti_tunnel_pos, _) =
            resolve_subtile_movement(start_pos, &hitbox, Vec2::new(48.0, 0.0), &map, &grid);
        assert_eq!(anti_tunnel_pos.0, 8.0);
    }

    #[test]
    fn test_bsp_dungeon_connectivity() {
        let config = BspDungeonConfig {
            width: 36,
            height: 28,
            max_depth: 3,
            min_leaf_size: 7,
            min_room_size: 4,
            tile_size_px: 16,
        };

        // Test determinism and Flood-Fill connectivity across multiple seeds
        for seed in [1_u64, 42, 1337, 987_654_321, 0xCAFE_BABE_DEAD_BEEF] {
            let d1 = generate_bsp_dungeon("dungeon.test", seed, &config).unwrap();
            let d2 = generate_bsp_dungeon("dungeon.test", seed, &config).unwrap();

            // Must be bit-identical for the same seed
            assert_eq!(d1, d2);
            assert!(d1.rooms.len() >= 2);
            assert_ne!(d1.spawn_tile, d1.exit_tile);

            // Spawn and Exit must be walkable
            assert_eq!(d1.collision_at(d1.spawn_tile), TileCollision::None);
            assert_eq!(d1.collision_at(d1.exit_tile), TileCollision::None);

            // Flood-Fill from spawn_tile must reach exit_tile and all room centers
            let reachable = d1.flood_fill_reachable_from(d1.spawn_tile);
            assert!(
                reachable.contains(&d1.exit_tile),
                "BSP dungeon seed {seed} failed Flood-Fill connectivity from spawn to exit"
            );
            assert!(d1.is_connected_spawn_to_exit());

            // Out-of-bounds must deterministically return TileCollision::Solid
            assert_eq!(d1.collision_at(TilePos(-1, -1)), TileCollision::Solid);
            assert_eq!(
                d1.collision_at(TilePos(config.width as i32, config.height as i32)),
                TileCollision::Solid
            );
        }
    }

    #[test]
    fn test_prefab_deserialization_success() {
        // 1. Load valid RON prefab from string
        let ron_prefab = concat!(
            "(\n",
            "    id: \"prefab.test_cottage\",\n",
            "    display_name: \"Test Cottage\",\n",
            "    width: 8,\n",
            "    height: 6,\n",
            "    rows: [\n",
            "        \"########\",\n",
            "        \"#..C...#\",\n",
            "        \"#..N...#\",\n",
            "        \"#..S...#\",\n",
            "        \"#..E...#\",\n",
            "        \"########\",\n",
            "    ],\n",
            "    legend: {\n",
            "        '#': \"tile.pastel_cobble\",\n",
            "        '.': \"tile.pastel_path\",\n",
            "        'S': \"tile.pastel_path\",\n",
            "        'E': \"tile.buttercup_sand\",\n",
            "    },\n",
            ")"
        );
        let state = PrefabLoader::load_from_str(ron_prefab, DataFormat::Ron, None).unwrap();
        assert_eq!(state.instance_id, "prefab.test_cottage");
        assert_eq!(state.width, 8);
        assert_eq!(state.height, 6);
        assert_eq!(state.spawn_tile, TilePos(3, 3));
        assert_eq!(state.exit_tile, TilePos(3, 4));
        assert_eq!(state.collision_at(TilePos(0, 0)), TileCollision::Solid);
        assert_eq!(state.collision_at(TilePos(3, 3)), TileCollision::None);
        assert!(state.is_connected_spawn_to_exit());

        // 2. Load valid asset files from disk (RON and JSON)
        let house_from_disk =
            PrefabLoader::load_from_file("assets/prefabs/starter_house.ron", None).unwrap();
        assert_eq!(house_from_disk.instance_id, "prefab.starter_house");
        assert!(house_from_disk.is_connected_spawn_to_exit());

        let shrine_from_disk =
            PrefabLoader::load_from_file("assets/prefabs/village_shrine.json", None).unwrap();
        assert_eq!(shrine_from_disk.instance_id, "prefab.village_shrine");
        assert_eq!(
            shrine_from_disk.collision_at(TilePos(1, 1)),
            TileCollision::Water
        );
    }

    #[test]
    fn test_prefab_deserialization_invalid_format() {
        // 1. Malformed syntax -> returns RonParse / JsonParse error without panicking
        let malformed_ron = "( id: \"broken\", width: 5, ";
        assert!(matches!(
            PrefabLoader::load_from_str(malformed_ron, DataFormat::Ron, None),
            Err(PrefabError::RonParse(_))
        ));

        // 2. Dimension mismatch (declared height 5, only 3 rows provided)
        let bad_rows = concat!(
            "(\n",
            "    id: \"prefab.bad_rows\",\n",
            "    display_name: \"Bad Rows\",\n",
            "    width: 4,\n",
            "    height: 5,\n",
            "    rows: [\"####\", \"#S.#\", \"####\"],\n",
            ")"
        );
        assert!(matches!(
            PrefabLoader::load_from_str(bad_rows, DataFormat::Ron, None),
            Err(PrefabError::RowCountMismatch { .. })
        ));

        // 3. Unknown tile ID in legend -> returns UnknownTileId error
        let unknown_tile_id = concat!(
            "(\n",
            "    id: \"prefab.bad_tile\",\n",
            "    display_name: \"Bad Tile\",\n",
            "    width: 4,\n",
            "    height: 4,\n",
            "    rows: [\"####\", \"#SE#\", \"#..#\", \"####\"],\n",
            "    legend: { '.': \"tile.non_existent_quantum_void\" },\n",
            ")"
        );
        assert!(matches!(
            PrefabLoader::load_from_str(unknown_tile_id, DataFormat::Ron, None),
            Err(PrefabError::UnknownTileId { .. })
        ));

        // 4. Unknown ASCII symbol in rows -> returns UnknownTileSymbol error
        let unknown_symbol = concat!(
            "(\n",
            "    id: \"prefab.bad_symbol\",\n",
            "    display_name: \"Bad Symbol\",\n",
            "    width: 4,\n",
            "    height: 4,\n",
            "    rows: [\"####\", \"#S?#\", \"#E.#\", \"####\"],\n",
            ")"
        );
        assert!(matches!(
            PrefabLoader::load_from_str(unknown_symbol, DataFormat::Ron, None),
            Err(PrefabError::UnknownTileSymbol { symbol: '?', .. })
        ));

        // 5. Fallback loader handles non-existent / corrupt files gracefully without panicking
        let fallback =
            PrefabLoader::load_from_file_or_fallback("assets/prefabs/does_not_exist.ron", None);
        assert_eq!(
            fallback.collision_at(fallback.spawn_tile),
            TileCollision::None
        );
    }
}
