//! Game Loop & State Machine (`MainMenu` <-> `InGame` <-> `Paused`).
//!
//! Built on Bevy's `States` system (`bevy_state`), managing scene setup,
//! entity lifecycle (`OnEnter` / `OnExit`), and per-state game loop updates.

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde::{Deserialize, Serialize};

use crate::palette_sprite::{
    IndexedPixelGrid, PaletteVerificationSwatches, PixelSprite, TileSprite, WorldPixelPos,
};

/// Primary application state machine for `watchout_pixls`.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum GameState {
    /// Initial title screen & palette/render preview menu.
    #[default]
    MainMenu,
    /// Active 2D pixel-art world simulation & gameplay loop.
    InGame,
    /// In-game pause overlay state.
    Paused,
}

/// High-level state transition commands triggered by keyboard input or UI/tests.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTransitionAction {
    /// Transitions from `MainMenu` (or `Paused`) into `InGame`.
    StartGame,
    /// Transitions from `InGame` into `Paused`, or from `Paused` back to `InGame`.
    TogglePause,
    /// Returns from `InGame` or `Paused` to `MainMenu`.
    ReturnToMainMenu,
}

/// Marker component for entities belonging exclusively to the `MainMenu` state.
#[derive(Component, Debug, Default)]
pub struct MainMenuEntity;

/// Marker component for entities belonging exclusively to the `InGame` state.
#[derive(Component, Debug, Default)]
pub struct InGameEntity;

/// Component attached to the playable Hero Pixl sprite in `InGame` state.
#[derive(Component, Debug, Clone)]
pub struct HeroController {
    pub content_id: String,
    pub speed_px_per_sec: f32,
}

/// Resource tracking deterministic game-loop tick counts across states.
#[derive(Resource, Debug, Default, Clone, PartialEq, Eq)]
pub struct GameLoopMetrics {
    pub main_menu_ticks: u64,
    pub in_game_ticks: u64,
    pub paused_ticks: u64,
}

/// Spawns the `MainMenu` visual elements when entering `GameState::MainMenu`.
pub fn setup_main_menu_scene_system(mut commands: Commands) {
    // Palette verification swatches along the top of the Main Menu (x=16, y=16, 8x8 each)
    commands.spawn((
        MainMenuEntity,
        PaletteVerificationSwatches {
            screen_x: 16,
            screen_y: 16,
            swatch_size: 8,
        },
    ));

    // Decorative meadow tile emblem and hero mascot preview in the center of the Main Menu
    commands.spawn((
        MainMenuEntity,
        WorldPixelPos::new(156.0, 116.0),
        TileSprite {
            grid: IndexedPixelGrid::pastel_meadow_tile_8x8(),
        },
    ));
    commands.spawn((
        MainMenuEntity,
        WorldPixelPos::new(156.0, 116.0),
        PixelSprite {
            grid: IndexedPixelGrid::hero_pixl_sprite_8x8(),
            z_order: 10,
        },
    ));
}

/// Cleans up `MainMenu` entities when leaving `GameState::MainMenu`.
pub fn cleanup_main_menu_scene_system(
    mut commands: Commands,
    query: Query<Entity, With<MainMenuEntity>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

/// Spawns the Phase 1 Render-Test tilemap, palette verification strip, and Hero sprite
/// when entering `GameState::InGame`.
pub fn setup_in_game_scene_system(mut commands: Commands) {
    // 1. Palette verification swatch strip at (8, 8) — each color renders an 8x8 block
    commands.spawn((
        InGameEntity,
        PaletteVerificationSwatches {
            screen_x: 8,
            screen_y: 8,
            swatch_size: 8,
        },
    ));

    // 2. Spawn an 8x6 patch of 8x8 tiles starting at world (128, 96)
    // Outer ring uses cobblestone tiles, inner floor uses pastel meadow tiles
    let meadow = IndexedPixelGrid::pastel_meadow_tile_8x8();
    let cobble = IndexedPixelGrid::pastel_cobble_tile_8x8();

    let origin_x = 128;
    let origin_y = 96;
    let cols = 8;
    let rows = 6;

    for row in 0..rows {
        for col in 0..cols {
            let is_border = row == 0 || row == rows - 1 || col == 0 || col == cols - 1;
            let grid = if is_border {
                cobble.clone()
            } else {
                meadow.clone()
            };
            let wx = (origin_x + col * 8) as f32;
            let wy = (origin_y + row * 8) as f32;
            commands.spawn((
                InGameEntity,
                WorldPixelPos::new(wx, wy),
                TileSprite { grid },
            ));
        }
    }

    // 3. Spawn the Hero Pixl Sprite at world (152, 112) inside the meadow courtyard
    commands.spawn((
        InGameEntity,
        HeroController {
            content_id: "entity.hero_pixl".to_string(),
            speed_px_per_sec: 64.0,
        },
        WorldPixelPos::new(152.0, 112.0),
        PixelSprite {
            grid: IndexedPixelGrid::hero_pixl_sprite_8x8(),
            z_order: 20,
        },
    ));
}

/// Cleans up `InGame` entities when returning to `MainMenu`.
pub fn cleanup_in_game_scene_system(
    mut commands: Commands,
    query: Query<Entity, With<InGameEntity>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

/// Processes `StateTransitionAction` events and updates `NextState<GameState>`.
pub fn handle_state_transitions_system(
    mut events: EventReader<StateTransitionAction>,
    current_state: Res<State<GameState>>,
    mut next_state: ResMut<NextState<GameState>>,
) {
    for action in events.read() {
        match (current_state.get(), action) {
            (GameState::MainMenu | GameState::Paused, StateTransitionAction::StartGame) => {
                next_state.set(GameState::InGame);
            }
            (GameState::InGame, StateTransitionAction::TogglePause) => {
                next_state.set(GameState::Paused);
            }
            (GameState::Paused, StateTransitionAction::TogglePause) => {
                next_state.set(GameState::InGame);
            }
            (GameState::InGame | GameState::Paused, StateTransitionAction::ReturnToMainMenu) => {
                next_state.set(GameState::MainMenu);
            }
            _ => {}
        }
    }
}

/// Increments the Main Menu tick counter during `GameState::MainMenu`.
pub fn main_menu_update_system(mut metrics: ResMut<GameLoopMetrics>) {
    metrics.main_menu_ticks += 1;
}

/// Increments the In-Game tick counter during `GameState::InGame`.
pub fn in_game_update_system(mut metrics: ResMut<GameLoopMetrics>) {
    metrics.in_game_ticks += 1;
}

/// Increments the Paused tick counter during `GameState::Paused`.
pub fn paused_update_system(mut metrics: ResMut<GameLoopMetrics>) {
    metrics.paused_ticks += 1;
}

/// Bevy Plugin managing the `GameState` state machine (`MainMenu`, `InGame`, `Paused`)
/// and state-specific game loop systems.
#[derive(Default)]
pub struct GameStatePlugin;

impl Plugin for GameStatePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<StatesPlugin>() {
            app.add_plugins(StatesPlugin);
        }

        app.init_state::<GameState>()
            .init_resource::<GameLoopMetrics>()
            .add_event::<StateTransitionAction>()
            .add_systems(OnEnter(GameState::MainMenu), setup_main_menu_scene_system)
            .add_systems(OnExit(GameState::MainMenu), cleanup_main_menu_scene_system)
            .add_systems(OnEnter(GameState::InGame), setup_in_game_scene_system)
            .add_systems(OnExit(GameState::InGame), cleanup_in_game_scene_system)
            .add_systems(
                Update,
                (
                    handle_state_transitions_system,
                    main_menu_update_system.run_if(in_state(GameState::MainMenu)),
                    in_game_update_system.run_if(in_state(GameState::InGame)),
                    paused_update_system.run_if(in_state(GameState::Paused)),
                ),
            );
    }
}
