# Watchout Pixls! 🌸🗡️

> A tiny, vibrant, open-world pixel-art adventure built for exploration, tinkering, and endless community mods.

**Watchout Pixls!** ist ein leichtgewichtiges, modulares 2D-Action-RPG in **Rust** auf Basis von **Bevy ECS**. Es kombiniert den nostalgischen Charme klassischer GameBoy-Farbwelten in sanften Pastelltönen mit modernen Architekturprinzipien: prozedural generierte Chunk-Welten, progressive Klassenentwicklung (vom kleinen Messer bis zum Laserschwert!) und eine erweiterbare Modding-Schnittstelle.

Das Spiel ist von Grund auf als offene Lern- und Spielwiese konzipiert: Jeder kann hineinschauen, den Code nachvollziehen, eigene Spielwelten erschaffen oder das Basisspiel über Datenpakete nahtlos modifizieren.

---

## 🎮 Vision & Gameplay-Kern

- **Pastell-Retro-Charme:** Gestochen scharfe Pixel-Art im Stil nostalgischer Handhelds, gerendert über eine Integer-skalierte, weichzeichnungsfreie Pixel-Pipeline.
- **Prozedurale Open World:** Eine unendliche, rasterbasierte Welt, die dynamisch in Chunks generiert, gestreamt und deterministisch per Seed reproduziert wird.
- **Dungeons & Entdeckungen:** Geheimnisvolle Höhlen, verlassene Häuser und seltene Schauplätze – von handgebauten Story-Räumen bis zu labyrinthischen, prozeduralen Gewölben mit Bossbegegnungen.
- **Progressive Evolution:** Wähle deine Klasse und steige über einen verzweigten Skilltree auf. Deine Ausrüstung wächst mit deinen Aufgaben – von einfachen Starterwaffen bis zu mächtigen Sci-Fi-Artefakten.
- **Respekt für deine Zeit:** Nahtloses Pausieren, automatisches Speichern im Hintergrund und faire Checkpoints für den Wiedereinstieg nach brenzligen Abenteuern.

---

## 🛠️ Technologie-Stack & Architektur

Wir setzen auf **Bevy (0.15)** in einem bewusst schlanken, fokussierten 2D-Setup (`default-features = false` mit `bevy_state`, `bevy_asset`, `bevy_color`, `multi_threaded`), kombiniert mit einer deterministischen Software-Rendering-Pipeline (`winit` + `softbuffer` + `image`):

- **Reines Bevy ECS:** Volle Unterstützung für `App`, `Plugin`, `States` (`OnEnter`, `OnExit`, `in_state`), `Component`, `Resource`, `Event` und parallele Systeme.
- **Kompakt & Unabhängig:** Durch den Verzicht auf schwere 3D-PBR- und Shader-Pipelines bleiben Binary-Größe und Kompilierzeiten minimal. Keine lästigen Systemabhängigkeiten (`alsa-sys`/`libudev-sys`) unter Linux.
- **Deterministisches Pixel-Perfect-Rendering:** Feste native Auflösung (`320x240` bzw. `480x270`) mit Nearest-Neighbor-Skalierung garantiert absolute Schärfe ohne Sub-Pixel-Artefakte und ermöglicht vollwertige Headless-Tests in CI/CD.

---

## 📦 Aktueller Stand (Phase 1)

1. **Fenster & Rendering ([`src/window_render.rs`](src/window_render.rs)):**
   - Native Retro-Auflösung (`320x240` Standard, `480x270` Widescreen-Modus).
   - `PixelPerfectCamera2d` mit Integer-Pixel-Snapping und erzwungenem `TextureFilterMode::Nearest`.
   - Feste ganzzahlige Skalierung (`3x` -> `960x720`, `4x` -> `1280x960`), kein freies Resizing oder Fullscreen zur Wahrung des Pixel-Verhältnisses.
2. **Pastell-Palette & Sprites ([`src/palette_sprite.rs`](src/palette_sprite.rs)):**
   - Sanfte GameBoy-Farbpalette (`PastelGameBoyPalette`) mit 4-, 8- und 16-Farben-Presets (`#FAF3E0`, `#B8E0D2`, `#80B9AD`, `#4A5859` plus Akzente).
   - Indizierte Pixel-Art-Strukturen (`IndexedPixelGrid`) für 8x8-Tiles und Helden-Sprites.
3. **Mod- & Content-Architektur ([`src/content.rs`](src/content.rs), [`src/modding.rs`](src/modding.rs)):**
   - Zentrales Trait `ContentDefinition` und typisierte `ContentRegistry<T>` für Items, Klassen, Entities und Tiles.
   - Datengetriebenes Laden über **RON** (`.ron`) und **JSON** (`.json`).
   - `ModExtension`-Trait für native Rust-Plugins und dynamisch ladbare Bibliotheken (`.dll`, `.so`, `.dylib` via `libloading`).
4. **State Machine ([`src/state.rs`](src/state.rs)):**
   - Robuste Bevy-Zustandsverwaltung (`GameState::MainMenu`, `GameState::InGame`, `GameState::Paused`).

---

## 🧩 Modding: Eigene Inhalte hinzufügen

Das Spiel unterscheidet zwischen Basisinhalt und Mods primär durch den Speicherort. Neue Items, Klassen oder Kacheln können ganz ohne Rust-Compiler als reine Datendatei hinterlegt werden.

### Beispiel: Eine neue Waffe per RON-Datei (`assets/content/items/laser_blade.ron`)

```ron
ItemDefinition(
    id: "mod_pixl_laser_blade",
    name: "Pastell-Laserklinge",
    description: "Eine uralte Klinge aus gebündeltem Licht. Summt leise in Minzgrün.",
    category: Weapon(
        damage: 42,
        attack_speed: 1.8,
        scaling_stat: Dexterity,
    ),
    sprite_index: 24,
    rarity: Legendary,
)
```

Sobald das Spiel startet, liest die `ContentRegistry` den Ordner `assets/content/` (sowie den externen `mods/`-Ordner) ein und schaltet die Waffe direkt im Spielgeschehen frei.

---

## 🚀 Installation & Kompilierung

### Voraussetzungen
- [Rust Toolchain](https://rustup.rs/) (Edition 2021, aktueller `stable`-Kanal)
- Unter Linux genügen Standard-Build-Tools (`build-essential` bzw. `base-devel`).

### Repository klonen
```bash
git clone [https://github.com/DEIN-NUTZERNAME/watchout-pixls.git](https://github.com/DEIN-NUTZERNAME/watchout-pixls.git)
cd watchout-pixls
```

### Spiel starten
```bash
# Normaler Start mit Desktop-Fenster
cargo run

# Schnellerer Start im Dev-Modus mit Optimierungen
cargo run --release

# Headless-Modus (z. B. für Server-Umgebungen oder CI-Tests ohne Monitor)
cargo run -- --headless
```

### Tests ausführen
```bash
cargo test
```

---

## 🗺️ Roadmap & Zukunftsmodule

- [x] **Phase 1:** Fenster, ECS-Pipeline, Pastell-Palette, Content-Traits.
- [ ] **Phase 2:** Prozedurales Chunk-Grid, Simplex-Noise-Biomgenerierung, Chunk-Streaming & binäre Serialisierung (`bincode`).
- [ ] **Phase 3:** Top-Down Controller mit Sub-Tile-Kollision, interaktive Kacheln, instanziierte Dungeon-Karten.
- [ ] **Phase 4:** Klassen-Fortschritt, Skilltree, Gegner-Zustandsmaschinen, Endboss-Mechanik & Checkpoint-Autosave.
- [ ] **Phase 5:** Autonomer Runtime-Mod-Loader (`/mods`), erweiterte Fehlerbehandlung und In-Game-Mod-Übersicht.

---

## 📜 Lizenz & Open-Source-Philosophie

Dieses Projekt ist lizenziert unter der **GNU General Public License v3.0 (GPLv3)**.

### Warum GPLv3?
Wir glauben fest an echte digitale Allmende:
- **Freiheit für alle:** Jeder darf den Quellcode einsehen, spielen, studieren, modifizieren und weiterverbreiten.
- **Schutz vor Vendor-Lock-in:** Niemand darf diesen Code nehmen, in ein geschlossenes ("Closed Source") Produkt verwandeln und Dritten vorenthalten.
- **Gemeinschaftlicher Mehrwert:** Wer Verbesserungen oder abgeleitete Werke auf Basis von *Watchout Pixls!* veröffentlicht, muss diese Modifikationen unter denselben freien Bedingungen der gesamten Community zurückgeben.

Details findest du in der beiliegenden Datei [`LICENSE`](LICENSE) oder online auf [gnu.org/licenses/gpl-3.0.html](https://www.gnu.org/licenses/gpl-3.0.html).
