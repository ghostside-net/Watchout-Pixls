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

## 📦 Aktueller Stand (Phase 1, Phase 2, Phase 3 & Phase 4)

1. **Fenster & Rendering ([`src/window_render.rs`](src/window_render.rs)):**
   - Native Retro-Auflösung (`320x240` Standard, `480x270` Widescreen-Modus).
   - `PixelPerfectCamera2d` mit Integer-Pixel-Snapping und erzwungenem `TextureFilterMode::Nearest`.
   - Feste ganzzahlige Skalierung (`3x` -> `960x720`, `4x` -> `1280x960`), kein freies Resizing oder Fullscreen zur Wahrung des Pixel-Verhältnisses.
2. **Pastell-Palette & Sprites ([`src/palette_sprite.rs`](src/palette_sprite.rs)):**
   - Sanfte GameBoy-Farbpalette (`PastelGameBoyPalette`) mit 4-, 8- und 16-Farben-Presets (`#FAF3E0`, `#B8E0D2`, `#80B9AD`, `#4A5859` plus Akzente).
   - Indizierte Pixel-Art-Strukturen (`IndexedPixelGrid`) für 8x8-Tiles und Helden-Sprites sowie Viewport-geculltes 16x16-Chunk-Rendering.
3. **Mod- & Content-Architektur ([`src/content.rs`](src/content.rs), [`src/modding.rs`](src/modding.rs)):**
   - Zentrales Trait `ContentDefinition` und typisierte `ContentRegistry<T>` für Items (`ItemDefinition`), Klassen (`ClassDefinition`), Entities (`EntityDefinition`), Tiles (`TileDefinition`) und Waffen (`WeaponDefinition`).
   - Datengetriebenes Laden über **RON** (`.ron`) und **JSON** (`.json`) inklusive mehrstufiger Waffenevolutionspfade (`WeaponTier::Starter` -> `Intermediate` -> `Advanced` -> `EndgameSciFi`).
   - `ModExtension`-Trait für native Rust-Plugins und dynamisch ladbare Bibliotheken (`.dll`, `.so`, `.dylib` via `libloading`).
4. **State Machine ([`src/state.rs`](src/state.rs)):**
   - Robuste Bevy-Zustandsverwaltung (`GameState::MainMenu`, `GameState::InGame`, `GameState::Paused`).
5. **Kern-Weltsystem ([`src/world/`](src/world/mod.rs)):**
   - **Koordinaten & Raster ([`src/world/coords.rs`](src/world/coords.rs)):** Tile-basiertes System (`16x16` Pixel pro Feld) und Einteilung in feste Chunks (`16x16` Tiles = 256 Tiles pro Chunk) mit euklidischer Koordinatenumrechnung (`TileCoord`, `TilePos`, `WorldPos`, `ChunkCoord`, `LocalTileCoord`) über alle positiven und negativen Quadranten.
   - **Prozedurale Weltgenerierung ([`src/world/noise.rs`](src/world/noise.rs), [`src/world/generation.rs`](src/world/generation.rs)):** Deterministischer 2D-Noise-Generator (`FastNoise2d`: Simplex, Perlin, FBm, Ridged Multifractal & SplitMix64-Tile-Hashing) auf Basis von `WorldSeed` für 5 Biome (`PastelMeadow`, `BreezeLake`, `LavenderForest`, `CobbleHighlands`, `SunlitGlade`), zusammenhängende Wege (`DirtPath`), Wände (`WallKind`) und abbaubare Ressourcen (`ResourceNode`).
   - **Streaming & Multithreading ([`src/world/streaming.rs`](src/world/streaming.rs)):** Dynamisches Laden aktiver Chunks im Radius um den Spieler via `bevy::tasks::AsyncComputeTaskPool` ohne Framedrops sowie Hysterese-basiertes Entladen entfernter Chunks.
   - **Persistenz & Speichersystem ([`src/world/persistence.rs`](src/world/persistence.rs)):** Effiziente Speicherung erforschter Gebiete (`ExploredChunkRecord`) und modifizierter Chunks (`ChunkData` & `TileDelta`) in kompaktem Binärformat (`WPX2` + CRC32 + Zlib) sowie komprimiertem/unkomprimiertem RON und JSON.
6. **Player Controller, Sub-Tile-Kollision & Instanziierte Szenen ([`src/player.rs`](src/player.rs), [`src/interaction.rs`](src/interaction.rs), [`src/scene.rs`](src/scene.rs)):**
   - **8-Wege-Movement & Sliding-Kollision ([`src/player.rs`](src/player.rs)):** Geschwindigkeitsnormalisierung bei Diagonalschritten, Sub-Tile-Bewegung (`WorldPos`) mit achsengetrennter AABB-Sliding-Kollision (`resolve_subtile_movement`), Anti-Tunneling-Sub-Stepping, Y-Sorting (`YSort`) und `PlayerState` (`Idle`, `Moving`, `Interacting`, `Transitioning`).
   - **Interaktions- & Trigger-System ([`src/interaction.rs`](src/interaction.rs)):** `Interactable`-Trait und `InteractableTrigger` mit Unterscheidung zwischen automatischen Tile-Triggern (`TriggerMode::Automatic`) und interaktiven Tür-/Truhen-/NPC-Triggern (`TriggerMode::Interactive`, Taste `E` / `Leertaste`).
   - **Szenen-Management, BSP-Dungeons & Statische Prefabs ([`src/scene.rs`](src/scene.rs)):** `SceneManager` (`Scene::Overworld(OverworldState)` <-> `Scene::Instance(DungeonState)`) mit exakter Speicherung des Overworld-Rückkehrpunkts (`return_position`), deterministischer BSP-Dungeon-Generator (`generate_bsp_dungeon`) inklusive Flood-Fill-Pfadvalidierung sowie `PrefabLoader` für RON/JSON-Innenräume ([`assets/prefabs/starter_house.ron`](assets/prefabs/starter_house.ron), [`assets/prefabs/village_shrine.json`](assets/prefabs/village_shrine.json)).
7. **Charakterprogression, Waffenevolution, Gegner-KI, Endboss & Speichersystem ([`src/progression.rs`](src/progression.rs), [`src/combat.rs`](src/combat.rs), [`src/save_checkpoint.rs`](src/save_checkpoint.rs)):**
   - **Data-Driven Klassen & Waffenevolution ([`src/content.rs`](src/content.rs), [`assets/content/weapons.ron`](assets/content/weapons.ron), [`assets/content/classes.ron`](assets/content/classes.ron)):** Mehrstufige Waffenpfade vom `Rostigen Dolch` (`weapon.rusty_dagger`) über `Eisen-Kurzschwert`, `Kobalt-Katana` und `Lichtbogen-Klinge` bis zum Sci-Fi-Endgame-Artefakt `Pastell-Laserschwert` (`weapon.laser_sword`) sowie Magier-Pfad bis zum `Quanten-Stab` (`weapon.quantum_staff`).
   - **XP, Leveling & Verzweigter Skilltree ([`src/progression.rs`](src/progression.rs)):** `CharacterProgression` mit quadratischer XP-Kurve, Attributpunkten (`Attack`, `Speed`, `Defense`, `Vitality`) und verzweigtem `SkillTree` (`Offense`, `Mobility`, `Defense`) für passive Stat-Boni und aktive Skills (`ActiveSkillKind::Dash`, `ActiveSkillKind::AreaAttack`).
   - **Gegner-Zustandsmaschine & Mehrphasiger Endboss ([`src/combat.rs`](src/combat.rs)):** Standard-Gegner (`EnemyController`) mit endlicher Zustandsmaschine (`EnemyAiState::Patrol`, `Chase`, `Attack`) sowie der mehrphasige Endboss `Chrono-Lich Overlord` (`BossController`, `BossPhase::Phase1` / `Phase2` / `Phase3`) mit rotierenden Angriffsmustern (`CleavingStrike`, `RadialPulse`, `ChargingLunge`, `SupernovaBurst`) und Level-Gate (`recommended_min_level = 5`).
   - **Auto-Save, Checkpoints & Pausenmenü ([`src/save_checkpoint.rs`](src/save_checkpoint.rs)):** Automatisches Speichern in festen Intervallen und bei Zonenwechseln (`AutoSaveConfig`, `SaveGameManager`), aktivierbare Karten-Checkpoints (`Checkpoint`, `ActiveCheckpoint`) für automatischen Tod-Respawn sowie `PauseMenuState` (`GameState::Paused`) mit manuellem Speichern und Laden (`GameSaveState` in RON/JSON).

---

## 🧩 Modding: Eigene Inhalte hinzufügen

Das Spiel unterscheidet zwischen Basisinhalt und Mods primär durch den Speicherort. Neue Items, Waffen, Klassen oder Kacheln können ganz ohne Rust-Compiler als reine Datendatei hinterlegt werden.

### Beispiel: Eine neue Waffe per RON-Datei (`assets/content/weapons.ron`)

```ron
[
    (
        id: "weapon.laser_sword",
        display_name: "Pastell-Laserschwert",
        tier: EndgameSciFi,
        base_damage: 62,
        attack_speed: 2.0,
        range_px: 26.0,
        crit_bonus: 0.22,
        required_level: 5,
        evolves_to: [],
        sprite_id: "sprite_laser_sword",
    ),
]
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
- [x] **Phase 2:** Prozedurales Chunk-Grid, Simplex-Noise-Biomgenerierung, Chunk-Streaming & binäre Serialisierung.
- [x] **Phase 3:** Top-Down Controller mit Sub-Tile-Kollision, interaktive Kacheln, instanziierte Dungeon-Karten.
- [x] **Phase 4:** Klassen-Fortschritt, Skilltree, Gegner-Zustandsmaschinen, Endboss-Mechanik & Checkpoint-Autosave.
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