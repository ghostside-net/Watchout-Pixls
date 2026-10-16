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

pub mod content;
#[cfg(feature = "desktop-window")]
pub mod desktop;
pub mod modding;
pub mod palette_sprite;
pub mod state;
pub mod window_render;

use std::path::Path;

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;

use crate::content::ContentPlugin;
use crate::modding::ModLoaderPlugin;
use crate::palette_sprite::{PaletteAndSpritePlugin, PastelGameBoyPalette};
use crate::state::GameStatePlugin;
use crate::window_render::{RetroWindowConfig, WindowAndRenderPlugin};

/// Modular Bevy `PluginGroup` bundling all Phase 1 core plugins for `watchout_pixls`.
#[derive(Default)]
pub struct WatchoutPixlsPlugins {
    pub window_config: RetroWindowConfig,
    pub palette: PastelGameBoyPalette,
}

impl WatchoutPixlsPlugins {
    #[must_use]
    pub fn new(window_config: RetroWindowConfig, palette: PastelGameBoyPalette) -> Self {
        Self {
            window_config,
            palette,
        }
    }
}

impl PluginGroup for WatchoutPixlsPlugins {
    fn build(self) -> PluginGroupBuilder {
        let mut group = PluginGroupBuilder::start::<Self>()
            .add(WindowAndRenderPlugin::new(self.window_config))
            .add(PaletteAndSpritePlugin::new(self.palette))
            .add(ContentPlugin)
            .add(GameStatePlugin);

        let default_pack = Path::new("assets/content");
        if default_pack.exists() {
            group = group.add(ModLoaderPlugin::new().with_content_pack_dir(default_pack));
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
}
