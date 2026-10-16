# Watchout Pixls!

A small open-world pixelart game to play, enjoy, edit, mod and work with :)

Ein extrem modulares, leichtgewichtiges 2D-Open-Source-Pixel-Art-Spiel in **Rust** auf Basis von **Bevy ECS** (Zielplattform: Windows Desktop & Multiplattform Linux/macOS).

---

## 1. Technologie-Stack & Architektur-Begründung

Wir nutzen **Bevy (aktuelle Version 0.15)** in einer **strikt modularen, leichtgewichtigen 2D-Konfiguration** (`default-features = false` mit `bevy_state`, `bevy_asset`, `bevy_color`, `multi_threaded`), gekoppelt mit einer deterministischen Pixel-Perfect-2D-Render-Pipeline (`winit` + `softbuffer` + `image`):

- **Echtes Bevy ECS & Plugin-Ökosystem:** Volle Unterstützung für Bevy `App`, `Plugin`, `PluginGroup`, `States` (`OnEnter`, `OnExit`, `in_state`), `Component`, `Resource`, `Event` und parallele Systeme.
- **Leichtgewichtig & Schnell:** Durch Verzicht auf den schweren 3D-PBR-/Shader-Stack bleiben Compile-Zeiten kurz, die Binary kompakt und die Ausführung frei von System-Header-Abhängigkeiten (`alsa-sys`/`libudev-sys`), während Windows, Linux und macOS nahtlos unterstützt werden.
- **Deterministisches Pixel-Perfect-Rendering:** Ein nativer Retro-Framebuffer (`320x240` bzw. `480x270`) mit striktem Nearest-Neighbor-Integer-Scaling verhindert Sub-Pixel-Artefakte und Weichzeichnung zu 100 % und lässt sich komplett headless in `cargo test` verifizieren.

---

## 2. Features in Phase 1

1. **Fenster & Pixel-Perfect 2D-Rendering ([`src/window_render.rs`](src/window_render.rs)):**
   - Feste native Retro-Auflösung (`320x240` Standard, `480x270` Widescreen-Preset).
   - `PixelPerfectCamera2d` mit Integer-Pixel-Snapping und erzwungenem `TextureFilterMode::Nearest` (keine Weichzeichnung/Blur).
   - Feste ganzzahlige Fensterskalierung (`3x` -> `960x720` bzw. `4x` -> `1280x960`), kein freies Resizing (`resizable: false`) und kein Fullscreen (`fullscreen: false`).
2. **Pastellfarbene GameBoy-Palette & Sprite/Tile-Lookup ([`src/palette_sprite.rs`](src/palette_sprite.rs)):**
   - `PastelGameBoyPalette` (4-, 8- und 16-Farben-Presets mit sanfter Pastell-DMG-Rampe `#FAF3E0`, `#B8E0D2`, `#80B9AD`, `#4A5859` sowie Pastell-Akzenten).
   - Indizierte Pixel-Art-Assets (`IndexedPixelGrid`) für 8x8-Tiles (`pastel_meadow_tile_8x8`, `pastel_cobble_tile_8x8`) und das 8x8-Held-Sprite (`hero_pixl_sprite_8x8`).
3. **Mod- & Erweiterungsarchitektur ([`src/content.rs`](src/content.rs), [`src/modding.rs`](src/modding.rs)):**
   - Zentrales Trait `ContentDefinition` und generische `ContentRegistry<T>` für **Items** (`ItemDefinition`), **Klassen** (`ClassDefinition`), **Entities** (`EntityDefinition`) und **Tiles** (`TileDefinition`).
   - Datengetriebenes Laden via **RON** (`.ron`) und **JSON** (`.json`) (siehe Beispiel-Modpack in [`assets/content/`](assets/content/)).
   - Trait `ModExtension` für statische Rust-Plugins sowie dynamisch zur Laufzeit geladene Shared Libraries (`.dll` / `.so` / `.dylib` über `libloading`).
4. **Game Loop & State Machine ([`src/state.rs`](src/state.rs)):**
   - Bevy-State-Machine (`GameState::MainMenu`, `GameState::InGame`, `GameState::Paused`) mit sauberen `OnEnter`-/`OnExit`-Systemen und Render-Test-Szene.

---

## 3. Installation, Start & Tests

### Voraussetzungen
- Rust Toolchain (`stable`, Edition 2021)

### Tests ausführen
```bash
cargo test
```

### Spiel starten
```bash
# Desktop-Fenster (oder automatischer Headless-Fallback ohne Display-Server)
cargo run

# Expliziter Headless-Verifikationslauf
cargo run -- --headless
```

### Release-Build erstellen
```bash
cargo build --release
```
