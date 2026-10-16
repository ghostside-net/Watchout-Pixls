//! Modular Plugin & Mod-Loading Architecture.
//!
//! Supports two complementary extension mechanisms:
//! 1. **Data-Driven Content Packs** via external RON (`.ron`) and JSON (`.json`) files.
//! 2. **Code Mods / Dynamic Plugins** implementing the [`ModExtension`] trait,
//!    registrable either statically at compile time or dynamically via shared
//!    libraries (`.dll` on Windows, `.so` on Linux, `.dylib` on macOS).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{ContentError, GameContentRegistries};

/// Errors that can occur when discovering or loading a mod.
#[derive(Debug, Error)]
pub enum ModError {
    #[error("Mod '{0}' is already registered")]
    DuplicateModId(String),
    #[error("Mod ID or name must not be empty")]
    InvalidMetadata,
    #[error("Content error while loading mod '{mod_id}': {source}")]
    Content {
        mod_id: String,
        #[source]
        source: ContentError,
    },
    #[error("I/O error while reading mod manifest at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Dynamic library loading error: {0}")]
    DynamicLibrary(String),
}

/// Metadata describing an external or built-in Mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMetadata {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
}

impl ModMetadata {
    pub fn validate(&self) -> Result<(), ModError> {
        if self.id.trim().is_empty() || self.name.trim().is_empty() {
            return Err(ModError::InvalidMetadata);
        }
        Ok(())
    }
}

/// Declarative manifest (`mod.ron` / `mod.json`) for external data-driven or hybrid mods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModManifest {
    pub metadata: ModMetadata,
    /// Relative paths to item definition files (`.ron` / `.json`).
    #[serde(default)]
    pub items_files: Vec<String>,
    /// Relative paths to class definition files (`.ron` / `.json`).
    #[serde(default)]
    pub classes_files: Vec<String>,
    /// Relative paths to entity definition files (`.ron` / `.json`).
    #[serde(default)]
    pub entities_files: Vec<String>,
    /// Relative paths to tile definition files (`.ron` / `.json`).
    #[serde(default)]
    pub tiles_files: Vec<String>,
    /// Optional relative path to a dynamic shared library (`.dll` / `.so` / `.dylib`).
    #[serde(default)]
    pub dynamic_library: Option<String>,
}

/// Core extension interface for mods that inject content definitions or ECS systems.
///
/// Can be implemented by compiled Rust plugins or exported from dynamic libraries
/// via the C-ABI symbol `watchout_create_mod_extension`.
pub trait ModExtension: Send + Sync + 'static {
    /// Returns the mod's identifying metadata.
    fn metadata(&self) -> ModMetadata;

    /// Called during content registration so the mod can add or override
    /// Items, Classes, Entities, and Tiles in [`GameContentRegistries`].
    fn register_content(&self, registries: &mut GameContentRegistries) -> Result<(), ModError>;

    /// Optional hook to register custom Bevy ECS components, resources, or systems.
    fn configure_app(&self, _app: &mut App) {}
}

/// Signature of the FFI entrypoint function exported by dynamic mod libraries:
/// `#[no_mangle] pub unsafe extern "C" fn watchout_create_mod_extension() -> *mut Box<dyn ModExtension>`
pub type CreateModExtensionFn = unsafe extern "C" fn() -> *mut Box<dyn ModExtension>;

/// Symbol name looked up when loading a dynamic shared-library mod.
pub const DYNAMIC_MOD_ENTRYPOINT_SYMBOL: &[u8] = b"watchout_create_mod_extension\0";

/// Resource tracking all active mods and loaded dynamic libraries.
#[derive(Resource, Default)]
pub struct ModManager {
    loaded_mods: Vec<ModMetadata>,
    extensions: Vec<Arc<dyn ModExtension>>,
    #[cfg(feature = "dynamic-mods")]
    libraries: Vec<Arc<libloading::Library>>,
}

impl ModManager {
    /// Creates an empty `ModManager`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the list of loaded mod metadata in registration order.
    #[must_use]
    pub fn loaded_mods(&self) -> &[ModMetadata] {
        &self.loaded_mods
    }

    /// Checks whether a mod with `mod_id` is currently loaded.
    #[must_use]
    pub fn is_loaded(&self, mod_id: &str) -> bool {
        self.loaded_mods.iter().any(|m| m.id == mod_id)
    }

    /// Registers an in-memory `ModExtension` and immediately applies its content
    /// to `GameContentRegistries`.
    pub fn register_extension(
        &mut self,
        extension: Arc<dyn ModExtension>,
        registries: &mut GameContentRegistries,
    ) -> Result<(), ModError> {
        let meta = extension.metadata();
        meta.validate()?;
        if self.is_loaded(&meta.id) {
            return Err(ModError::DuplicateModId(meta.id));
        }

        extension.register_content(registries)?;
        self.loaded_mods.push(meta);
        self.extensions.push(extension);
        Ok(())
    }

    /// Loads an external data-pack mod from a `ModManifest` and base directory,
    /// populating `GameContentRegistries` from the referenced `.ron` and `.json` files.
    pub fn load_manifest_from_dir(
        &mut self,
        manifest: &ModManifest,
        base_dir: impl AsRef<Path>,
        registries: &mut GameContentRegistries,
    ) -> Result<(), ModError> {
        manifest.metadata.validate()?;
        let mod_id = manifest.metadata.id.clone();
        if self.is_loaded(&mod_id) {
            return Err(ModError::DuplicateModId(mod_id));
        }

        let base = base_dir.as_ref();

        for rel in &manifest.items_files {
            registries
                .items
                .load_list_from_file(base.join(rel))
                .map_err(|source| ModError::Content {
                    mod_id: mod_id.clone(),
                    source,
                })?;
        }

        for rel in &manifest.classes_files {
            registries
                .classes
                .load_list_from_file(base.join(rel))
                .map_err(|source| ModError::Content {
                    mod_id: mod_id.clone(),
                    source,
                })?;
        }

        for rel in &manifest.entities_files {
            registries
                .entities
                .load_list_from_file(base.join(rel))
                .map_err(|source| ModError::Content {
                    mod_id: mod_id.clone(),
                    source,
                })?;
        }

        for rel in &manifest.tiles_files {
            registries
                .tiles
                .load_list_from_file(base.join(rel))
                .map_err(|source| ModError::Content {
                    mod_id: mod_id.clone(),
                    source,
                })?;
        }

        self.loaded_mods.push(manifest.metadata.clone());
        Ok(())
    }

    /// Dynamically loads a compiled shared library (`.dll` / `.so` / `.dylib`)
    /// exporting `watchout_create_mod_extension`, registers its content, and keeps
    /// the library handle alive for the lifetime of the `ModManager`.
    ///
    /// # Safety
    /// Loading a shared library executes foreign native code. Only load trusted mod binaries.
    #[cfg(feature = "dynamic-mods")]
    pub unsafe fn load_dynamic_library(
        &mut self,
        library_path: impl AsRef<Path>,
        registries: &mut GameContentRegistries,
    ) -> Result<ModMetadata, ModError> {
        let lib = unsafe {
            libloading::Library::new(library_path.as_ref().as_os_str())
                .map_err(|e| ModError::DynamicLibrary(e.to_string()))?
        };
        let constructor: libloading::Symbol<'_, CreateModExtensionFn> = unsafe {
            lib.get(DYNAMIC_MOD_ENTRYPOINT_SYMBOL)
                .map_err(|e| ModError::DynamicLibrary(e.to_string()))?
        };
        let raw_ptr = unsafe { constructor() };
        if raw_ptr.is_null() {
            return Err(ModError::DynamicLibrary(
                "Mod entrypoint returned null pointer".to_string(),
            ));
        }
        let outer_box: Box<Box<dyn ModExtension>> = unsafe { Box::from_raw(raw_ptr) };
        let arc_ext: Arc<dyn ModExtension> = Arc::from(*outer_box);
        let meta = arc_ext.metadata();

        self.register_extension(arc_ext, registries)?;
        self.libraries.push(Arc::new(lib));
        Ok(meta)
    }
}

/// Bevy Plugin that initializes the [`ModManager`] resource and loads any
/// configured startup mod extensions or content directories.
#[derive(Default)]
pub struct ModLoaderPlugin {
    initial_extensions: Vec<Arc<dyn ModExtension>>,
    content_pack_dirs: Vec<PathBuf>,
}

impl ModLoaderPlugin {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a static/in-memory `ModExtension` to be registered during plugin build.
    #[must_use]
    pub fn with_extension(mut self, extension: Arc<dyn ModExtension>) -> Self {
        self.initial_extensions.push(extension);
        self
    }

    /// Adds a content pack directory containing `mod.ron` to be loaded if present on disk.
    #[must_use]
    pub fn with_content_pack_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.content_pack_dirs.push(dir.into());
        self
    }
}

impl Plugin for ModLoaderPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GameContentRegistries>() {
            let mut reg = GameContentRegistries::new();
            let _ = reg.register_core_defaults();
            app.insert_resource(reg);
        }

        let mut mod_manager = ModManager::new();

        // Register initial code extensions and configure the Bevy App
        for ext in &self.initial_extensions {
            ext.configure_app(app);
        }

        app.world_mut()
            .resource_scope(|_world, mut registries: Mut<GameContentRegistries>| {
                for ext in &self.initial_extensions {
                    mod_manager
                        .register_extension(Arc::clone(ext), &mut registries)
                        .expect("Failed to register initial ModExtension");
                }

                #[cfg(feature = "ron-content")]
                for dir in &self.content_pack_dirs {
                    let manifest_path = dir.join("mod.ron");
                    if manifest_path.exists() {
                        if let Ok(raw) = std::fs::read_to_string(&manifest_path) {
                            if let Ok(manifest) = ron::from_str::<ModManifest>(&raw) {
                                let _ = mod_manager.load_manifest_from_dir(
                                    &manifest,
                                    dir,
                                    &mut registries,
                                );
                            }
                        }
                    }
                }
            });

        app.insert_resource(mod_manager);
    }
}
