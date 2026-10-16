//! Cross-Platform Desktop Window Runner (`winit` + `softbuffer`).
//!
//! Creates a fixed-size, non-resizable desktop window scaled by an integer
//! factor (e.g. `320x240` @ `3x` = `960x720`), steps the Bevy ECS `App`,
//! and blits the Nearest-Neighbor upscaled `PixelCanvas` buffer.

use std::num::NonZeroU32;
use std::rc::Rc;

use bevy::prelude::*;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowButtons, WindowId};

use crate::state::StateTransitionAction;
use crate::window_render::{PixelCanvas, RetroWindowConfig};

/// Builds the locked `WindowAttributes` from a validated `RetroWindowConfig`.
///
/// Enforces:
/// - Exact integer-scaled inner dimensions (`physical_width x physical_height`)
/// - `resizable = false` (no free window resizing)
/// - `fullscreen = None` and disabled maximize button
#[must_use]
pub fn build_locked_window_attributes(config: &RetroWindowConfig) -> WindowAttributes {
    let size = PhysicalSize::new(config.physical_width(), config.physical_height());
    Window::default_attributes()
        .with_title(config.title.clone())
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_fullscreen(None)
        .with_enabled_buttons(WindowButtons::CLOSE | WindowButtons::MINIMIZE)
}

struct DesktopAppRunner {
    app: App,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl DesktopAppRunner {
    fn new(app: App) -> Self {
        Self {
            app,
            window: None,
            surface: None,
        }
    }

    fn present_canvas(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return;
        };
        let canvas = self.app.world().resource::<PixelCanvas>();
        let Some(w) = NonZeroU32::new(canvas.physical_width()) else {
            return;
        };
        let Some(h) = NonZeroU32::new(canvas.physical_height()) else {
            return;
        };

        if surface.resize(w, h).is_ok() {
            if let Ok(mut buffer) = surface.buffer_mut() {
                let src = canvas.scaled_argb_buffer();
                if buffer.len() == src.len() {
                    buffer.copy_from_slice(src);
                    let _ = buffer.present();
                }
            }
        }
        window.request_redraw();
    }
}

impl ApplicationHandler for DesktopAppRunner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let config = self.app.world().resource::<RetroWindowConfig>().clone();
        let attrs = build_locked_window_attributes(&config);

        if let Ok(win) = event_loop.create_window(attrs) {
            let rc_win = Rc::new(win);
            if let Ok(context) = softbuffer::Context::new(Rc::clone(&rc_win)) {
                if let Ok(surface) = softbuffer::Surface::new(&context, Rc::clone(&rc_win)) {
                    self.surface = Some(surface);
                }
            }
            self.window = Some(rc_win);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Enter | KeyCode::Space) => {
                            self.app
                                .world_mut()
                                .send_event(StateTransitionAction::StartGame);
                        }
                        PhysicalKey::Code(KeyCode::KeyP) => {
                            self.app
                                .world_mut()
                                .send_event(StateTransitionAction::TogglePause);
                        }
                        PhysicalKey::Code(KeyCode::Backspace) => {
                            self.app
                                .world_mut()
                                .send_event(StateTransitionAction::ReturnToMainMenu);
                        }
                        PhysicalKey::Code(KeyCode::KeyW | KeyCode::ArrowUp) => {
                            self.app.world_mut().send_event(
                                crate::world::HeroMoveEvent::TranslatePixels {
                                    delta: Vec2::new(0.0, -16.0),
                                    check_collision: true,
                                },
                            );
                        }
                        PhysicalKey::Code(KeyCode::KeyS | KeyCode::ArrowDown) => {
                            self.app.world_mut().send_event(
                                crate::world::HeroMoveEvent::TranslatePixels {
                                    delta: Vec2::new(0.0, 16.0),
                                    check_collision: true,
                                },
                            );
                        }
                        PhysicalKey::Code(KeyCode::KeyA | KeyCode::ArrowLeft) => {
                            self.app.world_mut().send_event(
                                crate::world::HeroMoveEvent::TranslatePixels {
                                    delta: Vec2::new(-16.0, 0.0),
                                    check_collision: true,
                                },
                            );
                        }
                        PhysicalKey::Code(KeyCode::KeyD | KeyCode::ArrowRight) => {
                            self.app.world_mut().send_event(
                                crate::world::HeroMoveEvent::TranslatePixels {
                                    delta: Vec2::new(16.0, 0.0),
                                    check_collision: true,
                                },
                            );
                        }
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.app.update();
                self.present_canvas();
            }
            _ => {}
        }
    }
}

/// Checks if a graphical desktop display environment is available on the host OS.
#[must_use]
pub fn has_graphical_display() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// Launches the native OS desktop window event loop if a display server is available.
pub fn run_desktop_window(app: App) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = DesktopAppRunner::new(app);
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())
}
