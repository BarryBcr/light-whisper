# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
# Development (hot reload frontend, debug backend)
cargo tauri dev

# Production build (creates .app + .dmg on macOS)
cargo tauri build

# Debug build (faster, no bundling optimization)
cargo tauri build --debug

# Build just the Rust backend (no frontend bundling)
cd src-tauri && cargo build
```

**Prerequisites:** Rust toolchain, CMake, C/C++ compiler (for whisper.cpp compilation). First build takes ~5min due to whisper.cpp.

**macOS note:** `CMAKE_OSX_DEPLOYMENT_TARGET=11.0` is required for whisper.cpp's `std::filesystem` usage. This is set automatically via `src-tauri/.cargo/config.toml`.

**Linux note (Ubuntu 22.04+ / Debian 12+):** `sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libasound2-dev libxdo-dev cmake build-essential`

## Architecture

Tauri v2 app with Rust backend and vanilla JS frontend (no bundler, `withGlobalTauri: true`).

### Data Flow

```
Alt+Space → do_toggle_recording() → AudioRecorder.start()
  → cpal captures on dedicated thread → Arc<Mutex<Vec<f32>>>
  → emits "waveform-update" events to frontend

(fork BarryBcr) pendant l'enregistrement → decoupe::TranscripteurContinu
  → transcrit des morceaux de 12-30 s coupés aux pauses (coupure.rs)

Alt+Space again → arrêt du continu → AudioRecorder.stop()
  → transcription du seul reste → assemblage
  → derniere-dictee.txt → retour du focus VÉRIFIÉ (paste::attendre_focus)
  → paste::paste_text() (presse-papier + Cmd+V, pas de remise de l'ancien par défaut)
  → journal.log (une ligne par dictée)
```

### Threading Model

- **Main thread**: Tauri windows, tray, commands
- **Shortcut handler**: spawns a new thread per Alt+Space to avoid blocking
- **Recording thread**: dedicated thread owns `cpal::Stream` (not Send — cannot cross threads)
- **Model download**: async via Tokio

### Shared State

`AppState` holds three `Mutex`-wrapped fields (`recorder`, `whisper`, `config`) managed by Tauri. All access requires `.lock().unwrap()`.

### Backend Modules (src-tauri/src/)

| Module | Role |
|--------|------|
| `lib.rs` | Tauri setup, commands, tray, shortcut handler |
| `audio.rs` | cpal capture on dedicated thread, resample, WAV export |
| `stt.rs` | whisper-rs context management and transcription |
| `paste.rs` | presse-papier, Cmd+V (CGEvent), focus et `changeCount` via ObjC |
| `recording.rs` | cycle d'une dictée, états de l'overlay |
| `decoupe.rs` / `coupure.rs` | transcription au fil de l'eau ; `coupure.rs` est pur et partagé avec `bench/` |
| `journal.rs` | une ligne par dictée dans `journal.log` |
| `config.rs` | JSON config I/O, directory paths |
| `model_manager.rs` | HuggingFace model download with streaming progress |

### Frontend (src/)

Two Tauri windows, plain HTML/JS/CSS:
- **recorder** (index.html): 280×80 frameless transparent overlay with waveform canvas
- **settings** (settings.html): device/model/language config + model download UI

Communication: `invoke()` for commands, `event.listen()` for backend→frontend events.

### IPC Events

| Event | Direction | Payload |
|-------|-----------|---------|
| `waveform-update` | backend→frontend | `f32` RMS value |
| `recording-started` | backend→frontend | — |
| `recording-stopped` | backend→frontend | — |
| `download-progress` | backend→frontend | `{percent, downloaded_mb, total_mb}` |
| `download-complete` | backend→frontend | — |

## Key Constraints

- **cpal::Stream is not Send**: audio capture must stay on its spawning thread. `AudioRecorder` uses `unsafe impl Send/Sync` because only `Arc`-wrapped data crosses thread boundaries.
- **Whisper requires 16kHz mono f32**: recording captures at device native rate, then linear-interpolation resamples.
- **Transcription is blocking**: runs synchronously on the shortcut handler thread after recording stops.
- **whisper-rs 0.15 API**: `full_n_segments()` returns `c_int` (not Result); use `state.get_segment(i)` → `Option<WhisperSegment>` → `.to_str()`.
- **Collage** : l'origine remettait l'ancien presse-papier 100 ms après le Cmd+V ; les apps lentes (Electron, Chrome) collaient alors l'ANCIEN contenu. Désormais pas de remise par défaut, et si `restaurer_presse_papier` est activé, remise tardive protégée par `changeCount`.
- **Ne jamais transcrire un long bloc d'un coup** : banc du 17/11/2026 (M2 8 Go, Parakeet int8 CPU) : 60 s → 4 s, 180 s → 35 s avec fin inventée. D'où la découpe.
- **Build Mac de Boubacar** : `make dev-variant` / `make installer-dev` (SDKROOT, symboles CoreML résolus à l'exécution, raisons dans le Makefile). Variante Dev = feature `variante-dev` : `~/lightwhisper-dev/`, Ctrl+Alt+Space, modèles partagés dans `~/lightwhisper/models`.
- **Tests** : `make test` ; banc de vitesse : `make banc` (mode `--morceaux` pour la découpe).

## Storage Paths

```
~/lightwhisper/
├── config.json              # réglages (+ restaurer_presse_papier, transcription_par_morceaux)
├── derniere-dictee.txt      # filet de sécurité (menu « Copier la dernière dictée »)
├── journal.log              # une ligne par dictée : durées, focus, collage
├── models/ggml-{size}.bin   # Downloaded whisper.cpp models
└── temp/                    # Temporary WAV files (macOS/Linux)
```

Windows uses `%TEMP%\lightwhisper\` for temp files.
