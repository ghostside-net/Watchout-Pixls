# SKILLS.md — watchout_pixls

## 1. Projektspezifikation & Zielsetzung
- **Projektname:** watchout_pixls
- **Beschreibung / Ziel:** A small open-world pixelart game to play, enjoy, edit, mod and work with :)
- **Programmiersprache / Framework:** Rust
- **Zielplattform:** Multiplatform Desktop
- **Primärer Validierungsbefehl:** `cargo test`
- **Empfohlene Build-Tools:** cargo build, cargo build --release
- **Empfohlener Test-Runner:** cargo test

---

## 2. Architektur & Coding-Standards (Rust)
- **Code-Qualität & Best Practices:**
  - Speichersicherheit & Ownership, idiomatische Result<T, E> Typen, Clippy Warnungen auflösen, cargo fmt verwenden.
  - Schreibe sauberen, modularen und wartbaren Code nach den Best Practices für **Rust**.
  - Verwende konsistente Formatierung und sprechende Namenskonventionen.
  - Halte Methoden und Funktionen fokussiert und deterministisch testbar.
- **Zielumgebung (Multiplatform Desktop):**
  - Plattformübergreifende GUI-Frameworks (z. B. Tauri, Qt, Electron, Compose Multiplatform), modulare Backend-/Frontend-Trennung, automatisierte Bündelung für Windows, macOS und Linux.
  - Vermeide hardcodierte absolute Pfade außerhalb des Projektverzeichnisses.
- **Dokumentation:**
  - Pflege eine prägnante README.md mit Installations- und Startanweisungen.
  - Kommentiere komplexe Algorithmen oder Designentscheidungen verständlich.

---

## 3. Test- & Verifikationsstrategie
- **Automatisierte Validierung:**
  - Vor jedem Abschluss einer Aufgabe MUSS der Validierungsbefehl fehlerfrei durchlaufen:
    ```bash
cargo test
    ```
  - Schreibe begleitend Unit-Tests und Regressionstests für alle neuen Funktionalitäten.
- **Fehleranalyse:**
  - Tritt ein Fehler auf, wird die Ausgabe in `build_error.log` protokolliert.
  - Analysiere den Traceback/Compilerfehler sorgfältig und behebe die Ursache deterministisch.
- **Release-Builds & Artefakte:**
  - Nutze für Kompilierung und Paketierung die vorgesehenen Build-Tools (`cargo build, cargo build --release`).
  - Fertige Binaries und Installer müssen in standardisierten Ausgabeverzeichnissen (z. B. `dist/`, `target/release/`, `build/`) abgelegt werden.
