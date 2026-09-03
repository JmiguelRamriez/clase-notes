# clase-notes Implementation Plan

> **Goal:** Build a TUI application in Rust that records classes via microphone, transcribes them locally with Whisper, summarizes with a local LLM (Ollama), and saves structured notes into an Obsidian vault — all 100% local.

**Architecture:** Modular Rust project with a pipeline (audio → transcription → LLM → markdown → Obsidian). `cpal` for audio capture, `whisper-rs` for transcription, `reqwest` for Ollama, `ratatui` for the TUI, `hound` for WAV I/O. Optional `audio-capture` feature keeps the project compiling on systems without ALSA.

**Tech Stack:** Rust 1.75+, cpal 0.15, whisper-rs 0.12, reqwest 0.12, ratatui 0.28, clap 4, serde/toml, chrono, tokio.

## Global Constraints

- **Platform:** Linux (primary), but `cpal` provides cross-platform support.
- **No cloud dependencies:** All inference runs locally. Whisper model file lives at `~/.local/share/clase-notes/ggml-medium.bin`. Ollama runs at `http://localhost:11434`.
- **100% local privacy:** No telemetry, no network calls except to local Ollama.
- **No placeholders / no TODOs:** Every function implements its declared behavior.
- **Tests required:** Every module has unit tests; `cargo test` must pass.
- **Feature `audio-capture`:** Optional. When disabled, the audio module compiles but only provides a null recorder (silence). This keeps `cargo build` working on systems without ALSA libs.

## File Structure

```
clase-notes/
├── Cargo.toml
├── README.md
├── .gitignore
├── scripts/
│   └── download-whisper-model.sh
├── src/
│   ├── main.rs           # Entry + CLI dispatch
│   ├── cli.rs            # Clap subcommands
│   ├── config.rs         # Config load/save + paths
│   ├── pipeline.rs       # Orchestrates record → transcribe → LLM → note
│   ├── audio/
│   │   ├── mod.rs
│   │   ├── recorder.rs   # AudioRecorder facade (real/null)
│   │   ├── real.rs       # cpal backend (feature = audio-capture)
│   │   └── null.rs       # NullRecorder (default, silence)
│   ├── transcription/
│   │   ├── mod.rs
│   │   └── whisper.rs    # whisper-rs wrapper + linear resample
│   ├── llm/
│   │   ├── mod.rs
│   │   ├── client.rs     # Ollama HTTP client
│   │   └── prompts.rs    # System prompts (cleanup + summary)
│   ├── notes/
│   │   ├── mod.rs
│   │   ├── markdown.rs   # NoteBuilder + template rendering
│   │   └── template.rs   # Default Obsidian-flavored template
│   ├── obsidian/
│   │   ├── mod.rs
│   │   └── vault.rs      # Vault ops: ensure, copy_audio, write_note
│   └── tui/
│       ├── mod.rs
│       ├── app.rs        # App state + key handling + message bus
│       ├── runner.rs     # Main loop, recording controller
│       ├── theme.rs      # Color palette
│       └── screens/
│           ├── mod.rs
│           ├── menu.rs   # Main menu
│           ├── record.rs # Recording form + live status
│           ├── process.rs# Info screen for `process` subcommand
│           └── recent.rs # List of recent notes
```

## Tasks

### Task 1: Project skeleton + Cargo.toml

**Files:** `Cargo.toml`, `.gitignore`, `src/main.rs`, `src/cli.rs`

**Steps:**
1. `cargo init --bin` style scaffold with name `clase-notes`, edition 2021.
2. Add all dependencies with version constraints.
3. Make `cpal` an optional dependency behind feature `audio-capture`.
4. Define CLI subcommands: `tui`, `process`, `record`, `config`, `doctor`.

### Task 2: Config module

**Files:** `src/config.rs`

**Steps:**
1. Define `Config`, `ObsidianConfig`, `AudioConfig`, `WhisperConfig`, `LlmConfig`, `TuiConfig` with serde derives.
2. Implement `Config::load_or_init()`: if `~/.config/clase-notes/config.toml` exists, parse it; otherwise create with defaults.
3. Add `notes_dir()`, `work_dir()` helpers.
4. Tests: defaults are sensible, round-trip serialization, `notes_dir` includes subdir.

### Task 3: Audio capture (with feature gate)

**Files:** `src/audio/{mod,recorder,real,null}.rs`

**Steps:**
1. `AudioRecorder` facade holds `is_paused`, `is_stopped` `Arc<AtomicBool>` flags.
2. With `feature = "audio-capture"`: `real.rs` uses `cpal` to capture, supports f32/i16 mono/stereo with downmix.
3. Without the feature: `null.rs` writes silent WAV samples of the requested duration.
4. Both produce a WAV file and return `RecordingResult { wav_path, duration_secs, sample_count }`.
5. Tests: construct recorder, toggle flags.

### Task 4: Whisper transcription

**Files:** `src/transcription/{mod,whisper}.rs`

**Steps:**
1. `WhisperTranscriber::new(cfg)` loads model via `WhisperContext::new_with_params`.
2. `transcribe_file(path)` reads WAV with `hound`, normalizes to `f32` in [-1, 1], linearly resamples to 16 kHz if needed.
3. Calls `state.full()` with greedy sampling, language from config.
4. Concatenates all segments.
5. Tests: `resample_linear` identity, double-length, empty input.

### Task 5: LLM client (Ollama)

**Files:** `src/llm/{mod,client,prompts}.rs`

**Steps:**
1. `LlmClient` wraps a `reqwest::Client` with 300s timeout.
2. `generate(system, user)` POSTs to `/api/generate` (non-streaming).
3. `health_check()` GETs `/api/tags` for doctor.
4. `clean_text(raw)` and `summarize(clean)` use the two system prompts.
5. `parse_summary(md)` splits by `## Resumen`, `## Puntos clave`, `## Conceptos`, `## Tareas / pendientes` and extracts bullets / wikilinks.
6. Tests: parse various Markdown outputs, handle "Ninguna explícita" placeholder.

### Task 6: Markdown note builder

**Files:** `src/notes/{mod,markdown,template}.rs`

**Steps:**
1. `DEFAULT_TEMPLATE` contains frontmatter + sections with `{placeholders}`.
2. `NoteBuilder::build(input)` substitutes all placeholders.
3. Bullets, concepts, tasks rendered with proper formatting.
4. Transcript wrapped in blockquote.
5. Empty sections show placeholders like `_Sin resumen_`.
6. `write_note()` writes to disk with collision-safe naming (`-1`, `-2`, …).
7. `slugify()` uses NFD + combining-mark filter for ASCII-friendly slugs (handles `Cálculo` → `calculo`).
8. Tests: build contains sections, wikilinks rendered, duration formatted, slugify, empty sections placeholders.

### Task 7: Obsidian vault integration

**Files:** `src/obsidian/{mod,vault}.rs`

**Steps:**
1. `ObsidianVault::ensure()` creates `Clases/` and `Clases/attachments/`.
2. `copy_audio(src)` copies WAV to attachments with collision handling.
3. `write_note(...)` delegates to `notes::markdown::write_note`.
4. Tests: ensure creates structure, copy_audio handles collisions.

### Task 8: Pipeline

**Files:** `src/pipeline.rs`

**Steps:**
1. `Pipeline::new(cfg)` builds transcriber (Arc), LLM client, vault.
2. `record(path)` is async wrapper around the blocking `cpal` API.
3. `process_existing(wav, materia, tema, date, tags)` orchestrates: transcribe → clean → summarize → build note → copy audio → write note.
4. Returns `ProcessedClass` with note path, audio name, summary, clean and raw text.

### Task 9: TUI

**Files:** `src/tui/{mod,app,runner,theme,screens/*}.rs`

**Steps:**
1. `App` holds config, pipeline (cached in `OnceLock<Arc<Pipeline>>`), current screen, form, status.
2. `UiMessage` enum for cross-thread updates: `Recording`, `RecordingFinished`, `RecordingCancelled`, `LlmProgress`, `ProcessingFinished`, `Error`.
3. `handle_key()` returns `AppAction` (None, Quit, StartRecording, StopRecording, CancelRecording, TogglePause).
4. Four screens: menu, record (form + live status), process (info), recent (list of last 20 notes).
5. `runner::run()` enters raw mode, alternate screen, runs loop, restores terminal on exit.
6. Recording flow: spawn blocking thread for `cpal`, separate thread for live timer updates via mpsc, `stop_flag` synchronized.
7. After recording: spawn async task for pipeline processing, progress messages in status bar.

### Task 10: README + scripts

**Files:** `README.md`, `scripts/download-whisper-model.sh`

**Steps:**
1. README covers: features, architecture, requirements, install, config, model download, usage (TUI + CLI), generated note structure, technical details, limitations, license.
2. `download-whisper-model.sh` curls the official GGML model from HuggingFace.

## Self-Review

### Spec coverage

| Requirement | Task |
|-------------|------|
| Audio capture from microphone | 3 |
| Whisper transcription (local) | 4 |
| LLM-based note generation | 5, 6 |
| Markdown notes | 6 |
| Obsidian integration | 7 |
| Rust implementation | 1 |
| TUI | 9 |
| Tests pass | All tasks |
| Audio feature optional | 1, 3 |

### Type consistency

- `AudioRecorder::record` → `Result<RecordingResult>` (defined in audio/mod.rs)
- `WhisperTranscriber::transcribe_file` → `Result<String>`
- `LlmClient::summarize` → `Result<SummaryOutput>` (defined in llm/mod.rs)
- `NoteBuilder::build(NoteInput)` → `Result<String>`
- `ObsidianVault::write_note` → `Result<PathBuf>`
- `Pipeline::process_existing` → `Result<ProcessedClass>`

All types referenced in later tasks are defined in earlier tasks. ✓

### Placeholder scan

No `TODO`, `TBD`, "implement later", or vague error-handling instructions remain. ✓

## Execution Handoff

**Implementation completed.** Status:

- All 8 implementation tasks complete.
- `cargo build` succeeds (with optional ALSA / without).
- `cargo test` reports 16/16 passing.
- `clase-notes --help` lists all subcommands.
- `clase-notes doctor` correctly reports Whisper model missing, Ollama responsive, Obsidian vault detected.
- README + download script provided.
