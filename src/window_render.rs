//! Pixel-Perfect 2D Window & Rendering Pipeline.
//!
//! Enforces a fixed native retro resolution (e.g. `320x240` or `480x270`),
//! strict Nearest-Neighbor filtering (no bilinear blur), and fixed integer
//! window scaling (e.g. `3x` or `4x`) without free resizing or fullscreen.

use std::num::NonZeroU32;
use std::path::Path;

use bevy::prelude::*;
use image::{ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::palette_sprite::RgbaColor;

/// Errors that can occur during window configuration or rendering.
#[derive(Debug, Error)]
pub enum RenderError {
    #[error(
        "Invalid texture filter mode: bilinear/blur filtering is forbidden in pixel-perfect mode"
    )]
    BlurFilterForbidden,
    #[error("Free window resizing is forbidden by retro window constraints")]
    ResizingForbidden,
    #[error("Fullscreen mode is forbidden by retro window constraints")]
    FullscreenForbidden,
    #[error("Invalid native resolution: {0}x{1}")]
    InvalidResolution(u32, u32),
    #[error("Failed to export rendered frame to PNG: {0}")]
    PngExport(#[from] image::ImageError),
}

/// Supported native retro base resolutions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetroResolutionPreset {
    /// Classic 4:3 retro console resolution (320x240).
    Retro320x240,
    /// Widescreen 16:9 pixel-art resolution (480x270, clean integer divisor of 1920x1080).
    Widescreen480x270,
    /// Custom fixed retro dimensions.
    Custom { width: u32, height: u32 },
}

impl RetroResolutionPreset {
    /// Returns the native `(width, height)` in virtual retro pixels.
    #[must_use]
    pub const fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Retro320x240 => (320, 240),
            Self::Widescreen480x270 => (480, 270),
            Self::Custom { width, height } => (width, height),
        }
    }
}

/// Texture sampling filter mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextureFilterMode {
    /// Strict Nearest-Neighbor point sampling (sharp pixel edges, zero blur).
    #[default]
    Nearest,
    /// Linear interpolation (explicitly rejected by `RetroWindowConfig::validate`).
    Linear,
}

/// Fixed integer scaling factor for multiplying the native retro resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum IntegerScaling {
    /// 2x integer scale.
    Scale2x,
    /// 3x integer scale (e.g., 320x240 -> 960x720).
    #[default]
    Scale3x,
    /// 4x integer scale (e.g., 320x240 -> 1280x960 or 480x270 -> 1920x1080).
    Scale4x,
    /// Arbitrary positive integer scale factor.
    Custom(NonZeroU32),
}

impl IntegerScaling {
    /// Returns the integer multiplier as `u32`.
    #[must_use]
    pub const fn factor(self) -> u32 {
        match self {
            Self::Scale2x => 2,
            Self::Scale3x => 3,
            Self::Scale4x => 4,
            Self::Custom(n) => n.get(),
        }
    }
}

/// Resource defining the locked retro window and rendering parameters.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetroWindowConfig {
    /// Window title displayed in the OS titlebar.
    pub title: String,
    /// Native internal retro resolution before scaling.
    pub resolution: RetroResolutionPreset,
    /// Integer scaling multiplier applied to the window.
    pub scaling: IntegerScaling,
    /// Texture sampling filter (must be `TextureFilterMode::Nearest`).
    pub filter_mode: TextureFilterMode,
    /// Must remain `false` to prevent non-integer aspect distortion.
    pub resizable: bool,
    /// Must remain `false` to enforce windowed integer-scaled presentation.
    pub fullscreen: bool,
}

impl Default for RetroWindowConfig {
    fn default() -> Self {
        Self {
            title: "Watchout Pixls! (v0.1.0)".to_string(),
            resolution: RetroResolutionPreset::Retro320x240,
            scaling: IntegerScaling::Scale3x,
            filter_mode: TextureFilterMode::Nearest,
            resizable: false,
            fullscreen: false,
        }
    }
}

impl RetroWindowConfig {
    /// Creates a preset configuration for 480x270 widescreen with 3x integer scaling.
    #[must_use]
    pub fn widescreen_480x270_3x() -> Self {
        Self {
            resolution: RetroResolutionPreset::Widescreen480x270,
            scaling: IntegerScaling::Scale3x,
            ..Self::default()
        }
    }

    /// Native internal width in retro pixels.
    #[must_use]
    pub const fn native_width(&self) -> u32 {
        self.resolution.dimensions().0
    }

    /// Native internal height in retro pixels.
    #[must_use]
    pub const fn native_height(&self) -> u32 {
        self.resolution.dimensions().1
    }

    /// Scaled physical window width in desktop pixels (`native_width * scale_factor`).
    #[must_use]
    pub const fn physical_width(&self) -> u32 {
        self.native_width() * self.scaling.factor()
    }

    /// Scaled physical window height in desktop pixels (`native_height * scale_factor`).
    #[must_use]
    pub const fn physical_height(&self) -> u32 {
        self.native_height() * self.scaling.factor()
    }

    /// Validates that the configuration strictly obeys all retro rendering rules.
    pub fn validate(&self) -> Result<(), RenderError> {
        let (w, h) = self.resolution.dimensions();
        if w == 0 || h == 0 {
            return Err(RenderError::InvalidResolution(w, h));
        }
        if self.filter_mode != TextureFilterMode::Nearest {
            return Err(RenderError::BlurFilterForbidden);
        }
        if self.resizable {
            return Err(RenderError::ResizingForbidden);
        }
        if self.fullscreen {
            return Err(RenderError::FullscreenForbidden);
        }
        Ok(())
    }
}

/// ECS Component representing a Pixel-Perfect 2D Camera.
///
/// Even when logical world coordinates use `f32`, camera projection snaps
/// positions to integer pixel boundaries (`IVec2`) and enforces `Nearest` sampling.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct PixelPerfectCamera2d {
    /// Logical world position of the camera's top-left origin.
    pub position: Vec2,
    /// Native viewport size in retro pixels (e.g. `320x240`).
    pub viewport_size: UVec2,
    /// Sampling filter mode (locked to `TextureFilterMode::Nearest`).
    pub filter_mode: TextureFilterMode,
}

impl PixelPerfectCamera2d {
    /// Creates a new pixel-perfect 2D camera for the given native resolution.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            position: Vec2::ZERO,
            viewport_size: UVec2::new(width, height),
            filter_mode: TextureFilterMode::Nearest,
        }
    }

    /// Returns the integer-snapped camera origin in world pixel space.
    #[must_use]
    pub fn snapped_origin(&self) -> IVec2 {
        IVec2::new(
            self.position.x.round() as i32,
            self.position.y.round() as i32,
        )
    }

    /// Projects a world position (`Vec2`) into integer screen-space pixel coordinates
    /// relative to this camera's snapped origin.
    #[must_use]
    pub fn world_to_screen_pixel(&self, world_pos: Vec2) -> IVec2 {
        let world_snapped = IVec2::new(world_pos.x.round() as i32, world_pos.y.round() as i32);
        world_snapped - self.snapped_origin()
    }
}

/// Native retro framebuffer (`native_width x native_height`) with deterministic
/// Nearest-Neighbor integer scaling to the physical window resolution.
#[derive(Resource, Debug, Clone)]
pub struct PixelCanvas {
    native_width: u32,
    native_height: u32,
    scale_factor: u32,
    /// Native-resolution pixel buffer (row-major `native_width * native_height`).
    native_pixels: Vec<RgbaColor>,
    /// Integer-scaled physical output buffer (`physical_width * physical_height`) in `0x00RRGGBB` format for `softbuffer`.
    scaled_argb_buffer: Vec<u32>,
    /// Number of completed render frames.
    pub frame_count: u64,
}

impl PixelCanvas {
    /// Allocates a new `PixelCanvas` from the given `RetroWindowConfig`.
    #[must_use]
    pub fn from_config(config: &RetroWindowConfig) -> Self {
        let native_width = config.native_width();
        let native_height = config.native_height();
        let scale_factor = config.scaling.factor();
        let native_len = (native_width * native_height) as usize;
        let scaled_len = (config.physical_width() * config.physical_height()) as usize;

        Self {
            native_width,
            native_height,
            scale_factor,
            native_pixels: vec![RgbaColor::BLACK; native_len],
            scaled_argb_buffer: vec![0; scaled_len],
            frame_count: 0,
        }
    }

    /// Native canvas width in retro pixels.
    #[must_use]
    pub const fn native_width(&self) -> u32 {
        self.native_width
    }

    /// Native canvas height in retro pixels.
    #[must_use]
    pub const fn native_height(&self) -> u32 {
        self.native_height
    }

    /// Physical scaled width (`native_width * scale_factor`).
    #[must_use]
    pub const fn physical_width(&self) -> u32 {
        self.native_width * self.scale_factor
    }

    /// Physical scaled height (`native_height * scale_factor`).
    #[must_use]
    pub const fn physical_height(&self) -> u32 {
        self.native_height * self.scale_factor
    }

    /// Integer scale factor.
    #[must_use]
    pub const fn scale_factor(&self) -> u32 {
        self.scale_factor
    }

    /// Clears the entire native canvas with a single solid color.
    pub fn clear(&mut self, color: RgbaColor) {
        self.native_pixels.fill(color);
    }

    /// Sets a single pixel in native retro coordinates `(x, y)`.
    /// Out-of-bounds coordinates are safely clipped.
    pub fn set_pixel(&mut self, x: i32, y: i32, color: RgbaColor) {
        if x < 0 || y < 0 {
            return;
        }
        let (ux, uy) = (x as u32, y as u32);
        if ux < self.native_width && uy < self.native_height {
            let idx = (uy * self.native_width + ux) as usize;
            self.native_pixels[idx] = color;
        }
    }

    /// Reads a pixel at native retro coordinates `(x, y)`.
    #[must_use]
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<RgbaColor> {
        if x < self.native_width && y < self.native_height {
            let idx = (y * self.native_width + x) as usize;
            self.native_pixels.get(idx).copied()
        } else {
            None
        }
    }

    /// Draws a filled axis-aligned rectangle in native retro coordinates.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: RgbaColor) {
        for dy in 0..height as i32 {
            for dx in 0..width as i32 {
                self.set_pixel(x + dx, y + dy, color);
            }
        }
    }

    /// Draws an outlined axis-aligned rectangle with a specified border thickness.
    pub fn draw_rect_outline(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        color: RgbaColor,
        thickness: u32,
    ) {
        let t = thickness as i32;
        let w = width as i32;
        let h = height as i32;
        // Top and bottom borders
        self.fill_rect(x, y, width, thickness, color);
        self.fill_rect(x, y + h - t, width, thickness, color);
        // Left and right borders
        self.fill_rect(x, y + t, thickness, (height - thickness * 2).max(0), color);
        self.fill_rect(x + w - t, y + t, thickness, (height - thickness * 2).max(0), color);
    }

    /// Measures the total pixel width of a text string rendered with scale.
    #[must_use]
    pub fn measure_text_width(text: &str, scale: u32) -> i32 {
        let scale = scale.max(1) as i32;
        let char_count = text.chars().count() as i32;
        if char_count == 0 {
            0
        } else {
            char_count * 6 * scale - scale
        }
    }

    /// Draws a text string using the authentic 5x7 retro bitmap font.
    pub fn draw_text(&mut self, start_x: i32, start_y: i32, text: &str, color: RgbaColor, scale: u32) {
        let scale = scale.max(1);
        let mut cur_x = start_x;
        for ch in text.chars() {
            if ch == '\n' {
                cur_x = start_x;
                continue;
            }
            let glyph = retro_5x7_glyph(ch);
            for (row_idx, &row_bits) in glyph.iter().enumerate() {
                for col_idx in 0..5 {
                    if (row_bits & (1 << (4 - col_idx))) != 0 {
                        self.fill_rect(
                            cur_x + (col_idx as i32) * (scale as i32),
                            start_y + (row_idx as i32) * (scale as i32),
                            scale,
                            scale,
                            color,
                        );
                    }
                }
            }
            cur_x += 6 * (scale as i32);
        }
    }

    /// Draws horizontally centered text on the native canvas at vertical position `y`.
    pub fn draw_text_centered(&mut self, y: i32, text: &str, color: RgbaColor, scale: u32) {
        let width = Self::measure_text_width(text, scale);
        let start_x = (self.native_width as i32 - width) / 2;
        self.draw_text(start_x, y, text, color, scale);
    }

    /// Performs strict **Nearest-Neighbor Integer Upscaling** from the native
    /// retro framebuffer (`native_width x native_height`) into the physical
    /// window buffer (`physical_width x physical_height`).
    ///
    /// Every native pixel maps to an exact `scale_factor x scale_factor` block
    /// of identical physical pixels with zero interpolation or blur.
    pub fn upscale_nearest_neighbor(&mut self) {
        let phys_w = self.physical_width();
        let phys_h = self.physical_height();
        let scale = self.scale_factor;

        for py in 0..phys_h {
            let src_y = py / scale;
            let src_row_offset = src_y * self.native_width;
            let dst_row_offset = py * phys_w;

            for px in 0..phys_w {
                let src_x = px / scale;
                let color = self.native_pixels[(src_row_offset + src_x) as usize];
                self.scaled_argb_buffer[(dst_row_offset + px) as usize] = color.to_softbuffer_u32();
            }
        }
        self.frame_count += 1;
    }

    /// Returns a slice of the native retro pixels.
    #[must_use]
    pub fn native_pixels(&self) -> &[RgbaColor] {
        &self.native_pixels
    }

    /// Returns a slice of the integer-scaled physical `0x00RRGGBB` buffer.
    #[must_use]
    pub fn scaled_argb_buffer(&self) -> &[u32] {
        &self.scaled_argb_buffer
    }

    /// Reads a physical pixel from the upscaled buffer at `(phys_x, phys_y)`.
    #[must_use]
    pub fn get_scaled_pixel_u32(&self, phys_x: u32, phys_y: u32) -> Option<u32> {
        let phys_w = self.physical_width();
        let phys_h = self.physical_height();
        if phys_x < phys_w && phys_y < phys_h {
            let idx = (phys_y * phys_w + phys_x) as usize;
            self.scaled_argb_buffer.get(idx).copied()
        } else {
            None
        }
    }

    /// Exports the native retro framebuffer as a lossless PNG image.
    pub fn export_native_png(&self, path: impl AsRef<Path>) -> Result<(), RenderError> {
        let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(self.native_width, self.native_height);
        for y in 0..self.native_height {
            for x in 0..self.native_width {
                if let Some(c) = self.get_pixel(x, y) {
                    img.put_pixel(x, y, Rgba([c.r, c.g, c.b, c.a]));
                }
            }
        }
        img.save(path)?;
        Ok(())
    }
}

/// Startup system that spawns the primary `PixelPerfectCamera2d` entity.
pub fn spawn_pixel_perfect_camera_system(mut commands: Commands, config: Res<RetroWindowConfig>) {
    config
        .validate()
        .expect("RetroWindowConfig must satisfy pixel-perfect constraints");
    commands.spawn(PixelPerfectCamera2d::new(
        config.native_width(),
        config.native_height(),
    ));
}

/// Bevy Plugin that initializes the retro window configuration, native pixel canvas,
/// and the `PixelPerfectCamera2d`.
#[derive(Default)]
pub struct WindowAndRenderPlugin {
    pub config: RetroWindowConfig,
}

impl WindowAndRenderPlugin {
    #[must_use]
    pub fn new(config: RetroWindowConfig) -> Self {
        Self { config }
    }
}

impl Plugin for WindowAndRenderPlugin {
    fn build(&self, app: &mut App) {
        self.config
            .validate()
            .expect("Invalid RetroWindowConfig provided to WindowAndRenderPlugin");

        let canvas = PixelCanvas::from_config(&self.config);
        app.insert_resource(self.config.clone())
            .insert_resource(canvas)
            .add_systems(Startup, spawn_pixel_perfect_camera_system);
    }
}

/// Returns the 7-row bitmask for a given ASCII character in 5x7 pixel font format.
/// Each row is 5 bits wide (bits 4..0).
#[must_use]
pub fn retro_5x7_glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10011, 0b10101, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '/' => [0b00001, 0b00010, 0b00100, 0b00100, 0b01000, 0b10000, 0b10000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        '>' => [0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000],
        '<' => [0b00001, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0b00001],
        '[' => [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110],
        ']' => [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110],
        '|' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        '=' => [0, 0b11111, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        ',' => [0, 0, 0, 0, 0, 0b01100, 0b01000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        _ => [0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111],
    }
}
