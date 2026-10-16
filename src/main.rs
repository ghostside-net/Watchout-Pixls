//! Main entrypoint for `watchout_pixls` (Phase 1).

use bevy::prelude::*;
use watchout_pixls::build_watchout_app;
use watchout_pixls::content::GameContentRegistries;
use watchout_pixls::modding::ModManager;
use watchout_pixls::palette_sprite::PastelGameBoyPalette;
use watchout_pixls::state::{GameState, StateTransitionAction};
use watchout_pixls::window_render::{PixelCanvas, RetroWindowConfig};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Watchout Pixls (v{})", env!("CARGO_PKG_VERSION"));
        println!("Modular 2D retro pixel-art open-source game built on Bevy ECS\n");
        println!("USAGE:");
        println!("    watchout_pixls [OPTIONS]\n");
        println!("OPTIONS:");
        println!("    -h, --help        Print this help message");
        println!("    -V, --version     Print version information");
        println!(
            "        --headless    Run in headless mode (verification without opening a window)"
        );
        return;
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("watchout_pixls {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let force_headless = args.iter().any(|a| a == "--headless");

    let mut app = build_watchout_app();

    // Step 1: Run initial MainMenu tick
    app.update();

    // Print Phase 1 diagnostic summary
    {
        let cfg = app.world().resource::<RetroWindowConfig>();
        let palette = app.world().resource::<PastelGameBoyPalette>();
        let registries = app.world().resource::<GameContentRegistries>();
        let mods = app.world().resource::<ModManager>();
        let state = app.world().resource::<State<GameState>>();

        println!("============================================================");
        println!(" watchout_pixls — Phase 1 (Modular Bevy ECS + Pixel-Perfect)");
        println!("============================================================");
        println!(
            " Native Resolution : {}x{} px ({:?})",
            cfg.native_width(),
            cfg.native_height(),
            cfg.resolution
        );
        println!(
            " Window Scaling    : {}x Integer Scaling -> {}x{} px (Resizable: {}, Fullscreen: {})",
            cfg.scaling.factor(),
            cfg.physical_width(),
            cfg.physical_height(),
            cfg.resizable,
            cfg.fullscreen
        );
        println!(" Texture Filtering : {:?}", cfg.filter_mode);
        println!(
            " Active Palette    : '{}' ({} Pastel GameBoy colors)",
            palette.name,
            palette.len()
        );
        println!(
            " Loaded Content    : {} total ({} Items, {} Classes, {} Entities, {} Tiles)",
            registries.total_definitions(),
            registries.items.len(),
            registries.classes.len(),
            registries.entities.len(),
            registries.tiles.len()
        );
        println!(" Loaded Mods       : {}", mods.loaded_mods().len());
        println!(" Initial GameState : {:?}", state.get());
        println!("============================================================");
    }

    #[cfg(feature = "desktop-window")]
    if !force_headless && watchout_pixls::desktop::has_graphical_display() {
        if let Err(err) = watchout_pixls::desktop::run_desktop_window(app) {
            eprintln!("Desktop window exited with error: {err}");
        }
        return;
    }

    // Headless verification loop: transition MainMenu -> InGame and render the test scene
    app.world_mut().send_event(StateTransitionAction::StartGame);
    app.update();
    app.update();

    let state = *app.world().resource::<State<GameState>>().get();
    let canvas = app.world().resource::<PixelCanvas>();
    println!(
        " Headless Render-Test verified in state {:?} (rendered frames: {})",
        state, canvas.frame_count
    );
}
