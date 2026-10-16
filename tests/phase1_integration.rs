//! Integration tests for Phase 1 of `watchout_pixls`.

use std::sync::Arc;

use bevy::prelude::*;
use watchout_pixls::content::{
    ClassDefinition, ContentDefinition, DataFormat, EntityArchetype, EntityDefinition,
    GameContentRegistries, ItemCategory, ItemDefinition, TileDefinition,
};
use watchout_pixls::modding::{ModError, ModExtension, ModLoaderPlugin, ModManager, ModMetadata};
use watchout_pixls::palette_sprite::{IndexedPixelGrid, PastelGameBoyPalette, RgbaColor};
use watchout_pixls::state::{GameLoopMetrics, GameState, HeroController, StateTransitionAction};
use watchout_pixls::window_render::{
    IntegerScaling, PixelCanvas, PixelPerfectCamera2d, RetroResolutionPreset, RetroWindowConfig,
    TextureFilterMode,
};
use watchout_pixls::{build_watchout_app, build_watchout_app_with_config};

#[test]
fn integration_widescreen_480x270_4x_integer_scaling() {
    let config = RetroWindowConfig {
        title: "watchout_pixls 480x270 Test".to_string(),
        resolution: RetroResolutionPreset::Widescreen480x270,
        scaling: IntegerScaling::Scale4x,
        filter_mode: TextureFilterMode::Nearest,
        resizable: false,
        fullscreen: false,
    };
    let palette = PastelGameBoyPalette::pastel_deluxe_16();

    let mut app = build_watchout_app_with_config(config, palette);
    app.update();

    let canvas = app.world().resource::<PixelCanvas>();
    assert_eq!(canvas.native_width(), 480);
    assert_eq!(canvas.native_height(), 270);
    assert_eq!(canvas.scale_factor(), 4);
    assert_eq!(canvas.physical_width(), 1920);
    assert_eq!(canvas.physical_height(), 1080);
    assert_eq!(canvas.scaled_argb_buffer().len(), 1920 * 1080);
}

#[test]
fn integration_full_game_loop_and_png_render_verification() {
    let mut app = build_watchout_app();

    // Tick 1: MainMenu
    app.update();
    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::MainMenu
    );

    // Transition MainMenu -> InGame
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::InGame
    );

    let metrics = app.world().resource::<GameLoopMetrics>();
    assert!(metrics.main_menu_ticks >= 1);
    assert!(metrics.in_game_ticks >= 1);

    // Verify Hero entity exists in ECS world
    let mut hero_query = app.world_mut().query::<&HeroController>();
    let hero = hero_query.single(app.world());
    assert_eq!(hero.content_id, "entity.hero_pixl");

    // Verify PNG export of the native 320x240 pixel-perfect canvas
    let tmp_png = std::env::temp_dir().join("watchout_pixls_render_test.png");
    {
        let canvas = app.world().resource::<PixelCanvas>();
        canvas.export_native_png(&tmp_png).unwrap();
    }
    assert!(tmp_png.exists());
    let _ = std::fs::remove_file(&tmp_png);
}

struct SamplePluginMod;

impl ModExtension for SamplePluginMod {
    fn metadata(&self) -> ModMetadata {
        ModMetadata {
            id: "mod.integration_plugin".to_string(),
            name: "Integration Plugin Mod".to_string(),
            version: "0.1.0".to_string(),
            author: "IntegrationTest".to_string(),
            description: "Adds custom class, item, entity, and tile via ModExtension trait"
                .to_string(),
        }
    }

    fn register_content(&self, registries: &mut GameContentRegistries) -> Result<(), ModError> {
        let mod_id = self.metadata().id;
        registries
            .items
            .register(ItemDefinition {
                id: "item.emerald_lantern".to_string(),
                display_name: "Mint Emerald Lantern".to_string(),
                category: ItemCategory::Tool,
                max_stack: 1,
                value: 300,
                power_bonus: 4,
                sprite_id: "sprite_emerald_lantern".to_string(),
            })
            .map_err(|source| ModError::Content {
                mod_id: mod_id.clone(),
                source,
            })?;

        registries
            .classes
            .register(ClassDefinition {
                id: "class.lantern_keeper".to_string(),
                display_name: "Lantern Keeper".to_string(),
                base_health: 110,
                base_mana: 60,
                base_speed: 58.0,
                starting_items: vec!["item.emerald_lantern".to_string()],
                passive_trait: "GuidingLight".to_string(),
            })
            .map_err(|source| ModError::Content {
                mod_id: mod_id.clone(),
                source,
            })?;

        registries
            .entities
            .register(EntityDefinition {
                id: "entity.moth_familiar".to_string(),
                display_name: "Pastel Moth Familiar".to_string(),
                archetype: EntityArchetype::Critter,
                max_health: 25,
                move_speed: 72.0,
                hostile: false,
                sprite_id: "sprite_moth_familiar".to_string(),
                drop_table: vec![],
            })
            .map_err(|source| ModError::Content {
                mod_id: mod_id.clone(),
                source,
            })?;

        registries
            .tiles
            .register(TileDefinition {
                id: "tile.lantern_stone".to_string(),
                display_name: "Illuminated Cobble".to_string(),
                walkable: true,
                transparent: true,
                movement_cost: 1.0,
                sprite_id: "tile_lantern_stone".to_string(),
                custom_indices_8x8: Some(vec![1; 64]),
            })
            .map_err(|source| ModError::Content { mod_id, source })?;

        Ok(())
    }
}

#[test]
fn integration_mod_loader_plugin_and_external_packs() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(watchout_pixls::content::ContentPlugin)
        .add_plugins(
            ModLoaderPlugin::new()
                .with_extension(Arc::new(SamplePluginMod))
                .with_content_pack_dir("assets/content"),
        );

    app.update();

    let registries = app.world().resource::<GameContentRegistries>();
    let mod_manager = app.world().resource::<ModManager>();

    assert!(mod_manager.is_loaded("mod.integration_plugin"));
    assert!(mod_manager.is_loaded("mod.pastel_frontier"));

    // Verify plugin-registered content
    assert_eq!(
        registries
            .items
            .get("item.emerald_lantern")
            .unwrap()
            .display_name(),
        "Mint Emerald Lantern"
    );
    assert_eq!(
        registries
            .classes
            .get("class.lantern_keeper")
            .unwrap()
            .display_name(),
        "Lantern Keeper"
    );
    assert_eq!(
        registries
            .entities
            .get("entity.moth_familiar")
            .unwrap()
            .display_name(),
        "Pastel Moth Familiar"
    );
    assert_eq!(
        registries
            .tiles
            .get("tile.lantern_stone")
            .unwrap()
            .display_name(),
        "Illuminated Cobble"
    );

    // Verify RON + JSON pack-registered content
    assert!(registries.items.get("item.lavender_wand").is_some());
    assert!(registries.entities.get("entity.peach_wisp").is_some());
    assert!(registries.tiles.get("tile.sky_stream").is_some());

    // Also verify JSON list loading for tiles
    let mut custom_reg = GameContentRegistries::new();
    let json_tiles = r#"[
        {
            "id": "tile.moss_brick",
            "display_name": "Mossy Brick",
            "walkable": false,
            "transparent": false,
            "movement_cost": 10.0,
            "sprite_id": "tile_moss_brick",
            "custom_indices_8x8": null
        }
    ]"#;
    let count = custom_reg
        .tiles
        .load_list_from_str(json_tiles, DataFormat::Json)
        .unwrap();
    assert_eq!(count, 1);

    // Verify camera & pixel lookup types are directly usable
    let cam = PixelPerfectCamera2d::new(320, 240);
    assert_eq!(cam.filter_mode, TextureFilterMode::Nearest);
    let grid = IndexedPixelGrid::pastel_cobble_tile_8x8();
    let pal = PastelGameBoyPalette::pastel_dmg_8();
    assert_eq!(
        grid.sample_color(0, 0, &pal).unwrap(),
        Some(RgbaColor::from_hex(0x4A5859))
    );
}
