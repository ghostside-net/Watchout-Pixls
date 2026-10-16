//! Pastel GameBoy Color Palette & Indexed Pixel-Art Sprite/Tile Rendering.
//!
//! Provides an indexed 4-to-16 color Pastel GameBoy palette and deterministic
//! pixel-art lookup for 2D tiles and sprites without texture filtering artifacts.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::window_render::{PixelCanvas, PixelPerfectCamera2d};

/// Errors related to palette creation or indexed pixel lookup.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PaletteError {
    #[error("Palette size {0} is out of range (must contain between 4 and 16 colors)")]
    InvalidPaletteSize(usize),
    #[error("Palette index {0} is out of bounds for palette of size {1}")]
    IndexOutOfBounds(u8, usize),
    #[error("Sprite dimensions {width}x{height} do not match index buffer length {len}")]
    DimensionMismatch { width: u32, height: u32, len: usize },
}

/// 8-bit per channel RGBA color used for deterministic pixel-art lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const BLACK: Self = Self::rgb(0, 0, 0);

    /// Creates an opaque `RgbaColor` from RGB bytes.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates an `RgbaColor` with explicit alpha.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque `RgbaColor` from a 24-bit hex literal (`0xRRGGBB`).
    #[must_use]
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
            a: 255,
        }
    }

    /// Converts into `0x00RRGGBB` format expected by `softbuffer`.
    #[must_use]
    pub const fn to_softbuffer_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Converts into a Bevy `Color` (`Srgba`).
    #[must_use]
    pub fn to_bevy_color(self) -> Color {
        Color::srgba_u8(self.r, self.g, self.b, self.a)
    }
}

/// Resource holding a Pastel GameBoy color palette (4 to 16 colors).
///
/// Indices `0..4` form the classic 4-shade Pastel DMG GameBoy ramp
/// (lightest cream background -> pastel mint -> sage midtone -> deep slate ink),
/// while indices `4..8` (or up to 16) provide pastel accent colors for sprites and UI.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PastelGameBoyPalette {
    pub name: String,
    colors: Vec<RgbaColor>,
}

impl Default for PastelGameBoyPalette {
    fn default() -> Self {
        Self::pastel_dmg_8()
    }
}

impl PastelGameBoyPalette {
    pub const MIN_COLORS: usize = 4;
    pub const MAX_COLORS: usize = 16;

    /// Creates a validated palette with 4 to 16 colors.
    pub fn new(name: impl Into<String>, colors: Vec<RgbaColor>) -> Result<Self, PaletteError> {
        let len = colors.len();
        if !(Self::MIN_COLORS..=Self::MAX_COLORS).contains(&len) {
            return Err(PaletteError::InvalidPaletteSize(len));
        }
        Ok(Self {
            name: name.into(),
            colors,
        })
    }

    /// Classic 4-color Pastel GameBoy Pocket/DMG palette.
    #[must_use]
    pub fn pastel_dmg_4() -> Self {
        Self {
            name: "Pastel DMG-4".to_string(),
            colors: vec![
                RgbaColor::from_hex(0xFAF3E0), // 0: Cream Mist (Lightest / Highlight)
                RgbaColor::from_hex(0xB8E0D2), // 1: Mint Pastel (Light Midtone)
                RgbaColor::from_hex(0x80B9AD), // 2: Sage Leaf (Dark Midtone)
                RgbaColor::from_hex(0x4A5859), // 3: Deep Slate Berry (Darkest Contour/Ink)
            ],
        }
    }

    /// Default 8-color Pastel GameBoy palette (4-shade DMG ramp + 4 pastel accents).
    #[must_use]
    pub fn pastel_dmg_8() -> Self {
        Self {
            name: "Pastel GameBoy-8".to_string(),
            colors: vec![
                RgbaColor::from_hex(0xFAF3E0), // 0: Cream Mist (Background / Sprite Transparency Key)
                RgbaColor::from_hex(0xB8E0D2), // 1: Mint Pastel (Light Grass / Soft Fill)
                RgbaColor::from_hex(0x80B9AD), // 2: Sage Leaf (Mid Shading / Foliage)
                RgbaColor::from_hex(0x4A5859), // 3: Deep Slate Berry (Outlines & Eyes)
                RgbaColor::from_hex(0xF7C5CC), // 4: Peach Blossom (Hero Tunic / Flower Petals)
                RgbaColor::from_hex(0xD6CDEA), // 5: Lavender Haze (Magic / Stone Highlight)
                RgbaColor::from_hex(0xF9E79F), // 6: Buttercup Sun (Crown / Accents)
                RgbaColor::from_hex(0xA9DEF9), // 7: Sky Breeze (Water / Crystal)
            ],
        }
    }

    /// Extended 16-color Pastel GameBoy Deluxe palette.
    #[must_use]
    pub fn pastel_deluxe_16() -> Self {
        let mut colors = Self::pastel_dmg_8().colors;
        colors.extend_from_slice(&[
            RgbaColor::from_hex(0xE2F0CB), // 8: Tea Green
            RgbaColor::from_hex(0xFFDAC1), // 9: Apricot Nectar
            RgbaColor::from_hex(0xFFB7B2), // 10: Rose Melon
            RgbaColor::from_hex(0xC7CEEA), // 11: Periwinkle Cloud
            RgbaColor::from_hex(0x957DAD), // 12: Muted Plum
            RgbaColor::from_hex(0x6C757D), // 13: Warm Cobble Gray
            RgbaColor::from_hex(0xB5838D), // 14: Dusty Mauve
            RgbaColor::from_hex(0x353B3C), // 15: Deep Charcoal Shadow
        ]);
        Self {
            name: "Pastel GameBoy Deluxe-16".to_string(),
            colors,
        }
    }

    /// Number of colors in the palette.
    #[must_use]
    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// Always `false` since valid palettes have at least 4 colors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// Looks up a color by palette index (`0..len`).
    pub fn lookup(&self, index: u8) -> Result<RgbaColor, PaletteError> {
        self.colors
            .get(index as usize)
            .copied()
            .ok_or(PaletteError::IndexOutOfBounds(index, self.colors.len()))
    }

    /// Returns a slice of all colors in this palette.
    #[must_use]
    pub fn colors(&self) -> &[RgbaColor] {
        &self.colors
    }
}

/// An indexed 2D pixel-art asset (used for both Tiles and Sprites).
///
/// Each pixel stores a palette index (`u8`) rather than raw RGBA bytes,
/// allowing instant palette swapping and verifying pixel-perfect palette lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedPixelGrid {
    pub id: String,
    pub width: u32,
    pub height: u32,
    /// Row-major palette indices (`width * height`).
    pub indices: Vec<u8>,
    /// Optional transparent palette index (typically `Some(0)` for sprites, `None` for solid tiles).
    pub transparent_index: Option<u8>,
}

impl IndexedPixelGrid {
    /// Creates a new validated `IndexedPixelGrid`.
    pub fn new(
        id: impl Into<String>,
        width: u32,
        height: u32,
        indices: Vec<u8>,
        transparent_index: Option<u8>,
    ) -> Result<Self, PaletteError> {
        let expected_len = (width * height) as usize;
        if indices.len() != expected_len {
            return Err(PaletteError::DimensionMismatch {
                width,
                height,
                len: indices.len(),
            });
        }
        Ok(Self {
            id: id.into(),
            width,
            height,
            indices,
            transparent_index,
        })
    }

    /// Returns the palette index at local pixel coordinates `(x, y)`.
    #[must_use]
    pub fn index_at(&self, x: u32, y: u32) -> Option<u8> {
        if x < self.width && y < self.height {
            self.indices.get((y * self.width + x) as usize).copied()
        } else {
            None
        }
    }

    /// Resolves the `RgbaColor` at `(x, y)` using the provided `PastelGameBoyPalette`.
    /// Returns `Ok(None)` if the pixel matches `transparent_index`.
    pub fn sample_color(
        &self,
        x: u32,
        y: u32,
        palette: &PastelGameBoyPalette,
    ) -> Result<Option<RgbaColor>, PaletteError> {
        let Some(idx) = self.index_at(x, y) else {
            return Ok(None);
        };
        if self.transparent_index == Some(idx) {
            return Ok(None);
        }
        palette.lookup(idx).map(Some)
    }

    /// Blits this indexed grid onto the `PixelCanvas` at screen position `(screen_x, screen_y)`.
    pub fn blit_to_canvas(
        &self,
        canvas: &mut PixelCanvas,
        palette: &PastelGameBoyPalette,
        screen_x: i32,
        screen_y: i32,
    ) -> Result<(), PaletteError> {
        for py in 0..self.height {
            for px in 0..self.width {
                if let Some(color) = self.sample_color(px, py, palette)? {
                    canvas.set_pixel(screen_x + px as i32, screen_y + py as i32, color);
                }
            }
        }
        Ok(())
    }

    /// Built-in 8x8 Pastel Meadow Grass/Flower Tile (opaque, uses indices 0..=4).
    #[must_use]
    pub fn pastel_meadow_tile_8x8() -> Self {
        #[rustfmt::skip]
        let indices = vec![
            1, 1, 2, 1, 1, 1, 2, 1,
            1, 0, 1, 1, 2, 1, 1, 1,
            2, 1, 1, 4, 6, 4, 1, 2,
            1, 1, 2, 1, 4, 1, 1, 1,
            1, 2, 1, 1, 2, 1, 0, 1,
            1, 1, 1, 2, 1, 1, 1, 2,
            2, 1, 0, 1, 1, 2, 1, 1,
            1, 2, 1, 1, 1, 1, 2, 1,
        ];
        Self::new("tile_pastel_meadow", 8, 8, indices, None)
            .expect("8x8 meadow tile dimensions are valid")
    }

    /// Built-in 8x8 Retro Cobblestone Tile (opaque, uses indices 0, 2, 3, 5).
    #[must_use]
    pub fn pastel_cobble_tile_8x8() -> Self {
        #[rustfmt::skip]
        let indices = vec![
            3, 3, 3, 3, 3, 3, 3, 3,
            3, 0, 5, 5, 3, 0, 5, 3,
            3, 5, 5, 2, 3, 5, 2, 3,
            3, 3, 3, 3, 3, 3, 3, 3,
            3, 5, 3, 0, 5, 5, 2, 3,
            3, 2, 3, 5, 5, 2, 2, 3,
            3, 2, 3, 2, 2, 2, 3, 3,
            3, 3, 3, 3, 3, 3, 3, 3,
        ];
        Self::new("tile_pastel_cobble", 8, 8, indices, None)
            .expect("8x8 cobble tile dimensions are valid")
    }

    /// Built-in 8x8 Hero Pixl Adventurer Sprite (transparent index `0`).
    #[must_use]
    pub fn hero_pixl_sprite_8x8() -> Self {
        #[rustfmt::skip]
        let indices = vec![
            0, 0, 6, 6, 6, 6, 0, 0, // Pastel gold crown/hat
            0, 3, 3, 3, 3, 3, 3, 0, // Hat brim
            0, 3, 7, 1, 1, 7, 3, 0, // Eyes (7 = sky blue) & face (1 = mint)
            0, 0, 3, 1, 1, 3, 0, 0, // Chin
            0, 3, 4, 4, 4, 4, 3, 0, // Peach Blossom tunic
            3, 4, 4, 6, 6, 4, 4, 3, // Arms & gold belt buckle
            0, 0, 3, 2, 2, 3, 0, 0, // Sage cloak hem
            0, 0, 3, 0, 0, 3, 0, 0, // Boots
        ];
        Self::new("sprite_hero_pixl", 8, 8, indices, Some(0))
            .expect("8x8 hero sprite dimensions are valid")
    }
}

/// World-space pixel coordinates for 2D entities.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct WorldPixelPos {
    pub x: f32,
    pub y: f32,
}

impl WorldPixelPos {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn to_vec2(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

///ECS Component for a background/world Tile.
#[derive(Component, Debug, Clone)]
pub struct TileSprite {
    pub grid: IndexedPixelGrid,
}

/// ECS Component for a foreground Sprite entity.
#[derive(Component, Debug, Clone)]
pub struct PixelSprite {
    pub grid: IndexedPixelGrid,
    pub z_order: i32,
}

/// ECS Component that renders a color-palette verification bar on screen.
#[derive(Component, Debug, Clone, Copy)]
pub struct PaletteVerificationSwatches {
    pub screen_x: i32,
    pub screen_y: i32,
    pub swatch_size: u32,
}

/// System that renders all visible tiles, palette swatches, and sprites through
/// the `PixelPerfectCamera2d` onto the native `PixelCanvas`, then executes
/// strict Nearest-Neighbor integer scaling.
pub fn render_pixel_scene_system(
    palette: Res<PastelGameBoyPalette>,
    mut canvas: ResMut<PixelCanvas>,
    camera_query: Query<&PixelPerfectCamera2d>,
    tile_query: Query<(&WorldPixelPos, &TileSprite)>,
    swatch_query: Query<&PaletteVerificationSwatches>,
    sprite_query: Query<(&WorldPixelPos, &PixelSprite)>,
) {
    // 1. Clear native canvas with Palette Index 0 (Cream Mist)
    let bg_color = palette.lookup(0).unwrap_or(RgbaColor::BLACK);
    canvas.clear(bg_color);

    let camera = camera_query.iter().next().cloned().unwrap_or_else(|| {
        PixelPerfectCamera2d::new(canvas.native_width(), canvas.native_height())
    });

    // 2. Render background Tiles using integer-snapped camera coordinates
    for (pos, tile) in &tile_query {
        let screen_px = camera.world_to_screen_pixel(pos.to_vec2());
        let _ = tile
            .grid
            .blit_to_canvas(&mut canvas, &palette, screen_px.x, screen_px.y);
    }

    // 3. Render Palette Verification Swatches (if present in current scene)
    for swatches in &swatch_query {
        for (i, &color) in palette.colors().iter().enumerate() {
            let x = swatches.screen_x + (i as i32) * (swatches.swatch_size as i32);
            canvas.fill_rect(
                x,
                swatches.screen_y,
                swatches.swatch_size,
                swatches.swatch_size,
                color,
            );
        }
    }

    // 4. Render foreground Sprites sorted by z_order
    let mut sprites: Vec<(&WorldPixelPos, &PixelSprite)> = sprite_query.iter().collect();
    sprites.sort_by_key(|(_, s)| s.z_order);

    for (pos, sprite) in sprites {
        let screen_px = camera.world_to_screen_pixel(pos.to_vec2());
        let _ = sprite
            .grid
            .blit_to_canvas(&mut canvas, &palette, screen_px.x, screen_px.y);
    }

    // 5. Perform Nearest-Neighbor integer upscaling to physical window buffer
    canvas.upscale_nearest_neighbor();
}

/// Bevy Plugin registering the Pastel GameBoy palette and pixel-art rendering system.
#[derive(Default)]
pub struct PaletteAndSpritePlugin {
    pub palette: PastelGameBoyPalette,
}

impl PaletteAndSpritePlugin {
    #[must_use]
    pub fn new(palette: PastelGameBoyPalette) -> Self {
        Self { palette }
    }
}

impl Plugin for PaletteAndSpritePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.palette.clone())
            .add_systems(PostUpdate, render_pixel_scene_system);
    }
}
