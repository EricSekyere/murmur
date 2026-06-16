# Murmur - Product Requirements Document

## 1.0 Executive Summary

**Product Name:** Murmur
**Status:** Shipping on Windows (v0.3.6). See Section 1.5 for what is built versus still planned.
**Date:** Originally drafted February 18, 2026; last reviewed June 2026.

**Objective:** Build an open-source, privacy-first, cross-platform voice-to-text desktop tool designed for developers and general users. Murmur will accurately transcribe technical jargon, integrate natively with AI coding agents (Cursor, Claude Code, Gemini, etc.), map spoken directory references to real file paths, and work equally well for non-coding tasks like documentation, email, and chat.

**Problem Statement:**
Current voice dictation tools fall into two camps: (1) General-purpose tools (macOS Dictation, Windows Speech Recognition) that mangle technical terms -- turning "async await" into "a sink a weight" and "kubectl" into "cube cuddle"; and (2) Developer-specific tools (Wispr Flow, SuperWhisper, Willow Voice) that are proprietary, subscription-based, and often macOS-only.

No open-source tool today offers:
- Accurate transcription of programming syntax, frameworks, and CLI commands
- Intelligent directory/file path resolution from spoken references
- Native integration pipelines for multiple AI coding agents
- Equal macOS + Windows support
- Fully offline, local-first processing with zero cloud dependency

**Value Proposition:**
Murmur will enable developers to compose prompts for AI agents, write documentation, and communicate 3-4x faster than typing (150+ WPM speech vs 40 WPM average typing). It eliminates the context-switch tax of stopping to type, reduces RSI risk, and keeps proprietary code entirely on-device.

---

## 1.5 Current Implementation Status (v0.3.6)

This PRD was written as a forward-looking plan. The product has since shipped on Windows, but it took a different path than the original phases: dictation quality and live-preview UX were prioritized, while the heavier "code intelligence" and per-agent integration epics have not been built. This section is the source of truth for what exists today; the phase plans further down are kept for historical context and remaining direction.

### Shipped (Windows, v0.3.6)
- **Dictation.** Global-hotkey activation with toggle, push-to-talk, and double-tap modes. CPAL audio capture with Silero VAD (via ONNX Runtime).
- **STT.** whisper-rs (whisper.cpp) with base.en, small.en, medium.en, and large-v3-turbo, plus NVIDIA Parakeet TDT 0.6B v2 (ONNX). Default is small.en. GPU acceleration via CUDA on Windows; model switching in settings.
- **Output.** Direct Unicode typing into the focused application (Windows SendInput), with clipboard-and-paste available as an explicit output mode. The default path never routes through the clipboard, so prior clipboard contents cannot leak.
- **Live preview.** Interim text appears as you speak, shown in the dashboard and as a caption that sits either under the floating pill or near the active window (configurable). Each caption clears once its phrase is delivered.
- **Voice editing commands.** "select all", "copy/cut/paste that", "undo that", "redo that", "new line", "new paragraph", "scratch that", "press tab", "press escape".
- **Text snippets and custom vocabulary.** User-defined "trigger = expansion" snippets and a manual custom-vocabulary list that biases the decoder.
- **Multilingual and translate.** Non-English transcription and translate-to-English using the multilingual model.
- **Searchable history and per-app profiles.** Every delivered phrase is stored locally and tagged with the app it landed in; output mode and developer mode can switch automatically based on the focused application.
- **Find pill.** A control that flashes the floating widget and pulls it back on-screen if it drifted off a monitor.
- **App shell.** Tauri v2 system tray, dashboard, and first-run onboarding; sound feedback; TOML config with atomic writes, validation, corrupt-config recovery, and migration from the former "Voitex" name.
- **Distribution.** Signed auto-update (tauri-plugin-updater, minisign) and a GitHub release pipeline (tauri-action) producing the Windows build.

### Not yet built (still aspirational in this document)
- **Directory and path mapping** from spoken references (Section 5.5).
- **Codebase-derived vocabulary** via tree-sitter (Section 5.6); vocabulary today is a manual list, not auto-extracted.
- **Per-agent integrations:** VS Code / Cursor extension, Claude Code MCP server, and the local WebSocket API (Section 5.4, Epics 3.2 / 3.3 / 3.5). Integration today is the universal OS-level typing path plus CLI stdout piping.
- **Distinct coding / prose / command modes** (F6, Epic 2.4). A transcription profile setting exists, but not the full mode system.
- **Continuous always-listening mode and wake word** (Epic 4.2).

### Platform status
- **Windows (x64)** is the released, signed, auto-updating platform.
- **macOS** has platform-conditional code and a build target in the tree but is not yet signed or notarized, so it is not shipped (blocked on an Apple Developer account).
- **Linux** remains a stretch goal.

### CLI surface
`murmur listen`, `murmur config`, and `murmur models` (with stdout and clipboard output). The agent-oriented CLI surface from the plan (mcp-server, websocket) is not implemented.

---

## 2.0 Competitive Landscape

### 2.1 Existing Tools Analysis

| Tool | Type | Platforms | Offline | Code-Aware | Agent Integration | Price | Key Limitation |
|------|------|-----------|---------|------------|-------------------|-------|----------------|
| **Wispr Flow** | Commercial | macOS, Windows, iOS | Partial | Moderate | OS-level (universal) | Subscription | Closed-source, cloud-dependent for AI features |
| **SuperWhisper** | Commercial | macOS, iOS | Yes (whisper.cpp) | Low | OS-level (universal) | Subscription | macOS only, no Windows |
| **Willow Voice** | Commercial | macOS | Partial | High (variable recognition, file tagging) | Cursor-specific | Subscription | macOS only, YC startup (early stage) |
| **Talon Voice** | Free | macOS, Windows, Linux | Yes | High (via Cursorless) | Scriptable (Python) | Free | Extremely steep learning curve, not focused on dictation |
| **Serenade** | Open-source | macOS, Windows, Linux | Optional | High | VS Code, Chrome | Free | Less actively maintained, spotty recognition |
| **VS Code Speech** | Free extension | macOS, Windows, Linux | Yes | Low | VS Code only | Free | VS Code only, no code awareness |
| **WhisperFlow (OSS)** | Library | Any (Python) | Yes | None | None (library only) | Free | Library only, no desktop app, no code awareness |

### 2.2 Murmur Differentiation

| Capability | Wispr Flow | SuperWhisper | Talon | **Murmur** |
|-----------|------------|--------------|-------|-----------|
| Open source | No | No | No | **Yes** |
| Windows + macOS | Yes | No | Yes | **Yes** |
| Fully offline | Partial | Yes | Yes | **Yes** |
| Code jargon accuracy | Moderate | Low | High (manual config) | **High (automated)** |
| Directory path mapping | No | No | No | **Yes** |
| Agent-specific integrations | None | None | Scriptable | **Native (MCP, Extension API, stdin)** |
| Custom vocabulary from codebase | No | No | Manual | **Automated via tree-sitter** |
| Single binary distribution | Yes | Yes | No | **Yes** |

---

## 3.0 Target Users

### Primary
- **Software developers** who use AI coding agents (Cursor, Claude Code, Gemini, Windsurf, Amp) and want to compose prompts, describe features, and debug via voice
- **Developers with RSI/accessibility needs** who need a reliable, code-aware voice input method

### Secondary
- **Technical writers** creating API docs, README files, architecture docs
- **DevOps engineers** dictating infrastructure commands and configurations
- **General knowledge workers** who want fast, private voice dictation for email, chat, notes (the tool is not coding-only)

---

## 4.0 User Stories & Requirements

### 4.1 Functional Requirements

| ID | Category | User Story | Acceptance Criteria |
|----|----------|-----------|-------------------|
| F1 | **Core Transcription** | As a user, I want to press a hotkey, speak naturally, and see accurate text appear in my active application. | WER <8% on clean English speech. Text inserted within 300ms of speech end. Works in any application (editor, terminal, browser, chat). |
| F2 | **Technical Jargon** | As a developer, I want programming terms transcribed correctly without manual correction. | "async await", "kubectl apply", "useState", "OAuth two", "dot env", "npm install" all transcribed correctly. Custom vocabulary loaded from project codebase. |
| F3 | **Directory Mapping** | As a user, I want to say "the source components header file" and have `src/components/Header.tsx` inserted. | Tool indexes the current project directory. Fuzzy-matches spoken path segments to real paths. Supports user-defined aliases ("source" -> "src"). |
| F4 | **Agent Integration** | As a Cursor/Claude Code user, I want my voice transcription to appear directly in the agent's input. | Supports: OS-level input simulation (universal), VS Code Extension API (Cursor), stdin piping (Claude Code CLI), MCP server (Claude Code). |
| F5 | **Voice Commands** | As a user, I want to say formatting commands like "new line", "code block", "bullet point". | Reliably distinguishes commands from dictation. Commands execute inline (no separate mode required). |
| F6 | **Modes** | As a user, I want different modes: coding mode (tech vocabulary biased), prose mode (natural writing), command mode (short actions). | Mode switching via voice command or hotkey. Each mode adjusts vocabulary bias and formatting behavior. |
| F7 | **Privacy** | As a developer handling proprietary code, I need all processing to happen locally. | Zero network calls in default mode. All models run on-device. Optional cloud mode is opt-in with clear disclosure. No telemetry by default. |
| F8 | **General Use** | As a non-developer, I want to use this tool for everyday dictation in any app. | Prose mode provides clean, punctuated output suitable for email, Slack, documents. Auto-capitalization, filler word removal. |

### 4.2 Non-Functional Requirements

| ID | Category | Requirement |
|----|----------|------------|
| NF1 | **Latency** | End-to-end latency <300ms from speech end to text insertion on modern hardware (M1+ Mac, i7+ Windows). |
| NF2 | **Memory** | Idle memory <100MB. Active transcription <500MB (small model) or <2GB (large model). |
| NF3 | **Platform** | macOS (Apple Silicon + Intel) and Windows (x64) as equal first-class targets. Linux as future stretch goal. |
| NF4 | **Distribution** | Single binary installer. No Python runtime, no Node.js, no Docker required. macOS: .dmg. Windows: .msi or portable .exe. |
| NF5 | **Language** | English at launch. Multilingual support (99+ languages via Whisper) as a future milestone. |
| NF6 | **CPU/GPU** | Must work on CPU-only machines. GPU acceleration (CUDA, Metal, Vulkan) as optional performance boost. |
| NF7 | **Startup** | Cold start <3 seconds. Model loading <5 seconds for small model. |

---

## 5.0 Technical Architecture

### 5.1 Architecture Overview

```
+------------------------------------------------------------------+
|                        MURMUR CORE (Rust)                        |
|                                                                  |
|  +------------------+    +------------------+    +-----------+   |
|  | Audio Capture    |    | STT Engine       |    | Output    |   |
|  | (CPAL)           |--->| (whisper.cpp     |--->| Manager   |   |
|  |                  |    |  via whisper-rs)  |    |           |   |
|  | +-------------+  |    |                  |    | - Clipboard|  |
|  | | Silero VAD  |  |    | Models:          |    | - Keystroke|  |
|  | | (ONNX)      |  |    | - base.en (fast) |    | - stdin   |  |
|  | +-------------+  |    | - small.en       |    | - MCP     |  |
|  +------------------+    | - medium         |    +-----------+   |
|                          | - large-v3-turbo |                    |
|  +------------------+    +------------------+    +-----------+   |
|  | Code-Aware       |                           | Integration|  |
|  | Processor         |<----- raw text --------->| Layer      |   |
|  |                  |                           |            |   |
|  | - Command parser |    +------------------+    | - Cursor   |  |
|  | - Formatter      |    | Project Indexer  |    | - Claude   |  |
|  |   (camel/snake)  |    | (tree-sitter +   |    | - Gemini   |  |
|  | - Path resolver  |<---| file watcher)    |    | - Terminal |  |
|  | - Custom vocab   |    +------------------+    +-----------+   |
|  +------------------+                                            |
|                                                                  |
|  +------------------+    +------------------+                    |
|  | System Tray UI   |    | Config Manager   |                    |
|  | (Tauri)          |    | (TOML/JSON)      |                    |
|  +------------------+    +------------------+                    |
+------------------------------------------------------------------+
```

### 5.2 Why Rust (Disagreement with Previous PRD)

The previous PRD stated "Python is the unequivocal choice." **This is wrong for a desktop tool.** Here's why Rust is the correct primary language:

| Factor | Python | Rust | Winner |
|--------|--------|------|--------|
| **Binary distribution** | Requires Python runtime (500MB+) or PyInstaller (brittle, bloated) | Single static binary, no runtime | **Rust** |
| **Audio capture latency** | PyAudio/sounddevice add GIL overhead; can't guarantee real-time | CPAL provides direct OS audio API access, zero GC pauses | **Rust** |
| **Memory usage** | High baseline (~30MB runtime), GC spikes | Predictable, minimal overhead | **Rust** |
| **Cross-platform** | Works but distribution is painful on each OS | Compile once per target, consistent behavior | **Rust** |
| **STT inference** | Native faster-whisper is fast | whisper.cpp via whisper-rs is equally fast, no Python overhead | **Tie** |
| **Input simulation** | `keyboard` lib requires root on Linux, fragile | `enigo` is well-maintained, cross-platform | **Rust** |
| **ML ecosystem** | Far larger (PyTorch, HuggingFace) | Growing (ONNX Runtime, whisper-rs, candle) | **Python** |
| **Prototyping speed** | Faster iteration | Slower iteration, stricter compiler | **Python** |

**Decision:** Rust for the core engine (audio, VAD, STT, input simulation, CLI). Python only as an optional sidecar for experimental ML features. This matches the architecture of successful tools: SuperWhisper uses whisper.cpp (C++), Murmur uses Rust+Tauri+whisper-rs.

### 5.3 Core Technology Stack

#### Audio Capture
| Component | Library | License | Rationale |
|-----------|---------|---------|-----------|
| **Microphone input** | **CPAL** (Rust) | Apache 2.0 | Pure Rust, supports WASAPI (Windows), CoreAudio (macOS), ALSA/JACK (Linux). No C dependencies. |
| **Voice Activity Detection** | **Silero VAD** (via ONNX Runtime) | MIT | 4x more accurate than WebRTC VAD (87.7% vs 50% TPR at 5% FPR). Uses only 0.43% CPU. Trained on 6000+ languages. |

> **Note:** The previous PRD recommended WebRTC VAD. Research shows Silero VAD catches speech frames WebRTC misses entirely -- WebRTC drops 1 in 2 speech frames vs Silero's 1 in 8. For a coding tool where every word matters, this is critical.

#### Speech-to-Text Engine

| Model | WER | Speed | Size | Use Case |
|-------|-----|-------|------|----------|
| **whisper.cpp base.en** | ~10% | Ultra-fast, real-time on CPU | 142MB | Default for low-latency dictation |
| **whisper.cpp small.en** | ~8% | Fast, real-time on modern CPU | 466MB | Balanced mode |
| **whisper.cpp medium.en** | ~7% | Near real-time with GPU | 1.5GB | High-accuracy mode |
| **whisper.cpp large-v3-turbo** | ~7.75% | Requires GPU for real-time | 1.6GB | Maximum accuracy mode |
| **Distil-Whisper** | ~8% | 6x faster than large-v3 | 756MB | Future: best edge performance |
| **NVIDIA Parakeet TDT 0.6B v2** | ~6% | RTFx >2000 | ~660MB | Shipped: native punctuation and capitalization, English-only (ONNX) |

**Primary recommendation:** Start with **whisper.cpp small.en** via `whisper-rs`. It delivers <8% WER with real-time CPU performance and fits in <500MB RAM. Users can switch models in settings.

> **Disagreement with previous PRD:** The previous PRD recommended starting with Whisper Large V3 Turbo. This is too heavy for an MVP -- it requires GPU for real-time performance and uses 1.6GB+ RAM. Starting with `small.en` gives instant usability on any machine.

#### Code-Aware Processing

| Component | Library | License | Purpose |
|-----------|---------|---------|---------|
| **Code parsing** | **tree-sitter** (Rust bindings) | MIT | Parse project files to extract identifiers, function names, class names for vocabulary building |
| **Fuzzy matching** | **strsim** (Rust) | MIT | Match spoken words to codebase identifiers (Levenshtein, Jaro-Winkler) |
| **File watching** | **notify** (Rust) | CC0/Artistic 2.0 | Watch project directory for changes, update index |
| **Path resolution** | Custom (Rust) | -- | Phonetic + fuzzy matching against file index |

> **Disagreement with previous PRD:** The previous PRD suggested running Gemma-2B LLM for post-processing every transcription. This adds 500ms-2s latency and 2-4GB memory. For real-time coding dictation, a deterministic pipeline (custom vocabulary + pattern matching + fuzzy resolution) is faster and more predictable. LLM refinement should be an optional "polish" step, not the default path.

#### Integration & Output

| Component | Library | License | Purpose |
|-----------|---------|---------|---------|
| **Input simulation** | **enigo** (Rust) | MIT | Cross-platform keystroke/mouse simulation |
| **Clipboard** | **arboard** (Rust) | Apache 2.0/MIT | Cross-platform clipboard access |
| **Global hotkeys** | **global-hotkey** (Rust) | Apache 2.0/MIT | Register system-wide hotkeys |
| **System tray** | **Tauri** | Apache 2.0/MIT | Lightweight desktop UI, system tray, settings |
| **CLI interface** | **clap** (Rust) | Apache 2.0/MIT | CLI for piping, scripting, agent integration |

#### Configuration & Storage

| Component | Library | License | Purpose |
|-----------|---------|---------|---------|
| **Config format** | **TOML** (via `toml` crate) | MIT | Human-readable config files |
| **Custom vocabulary** | JSON files | -- | User-defined term mappings and aliases |
| **Path aliases** | JSON files | -- | "source" -> "src", project-specific mappings |

### 5.4 Integration Architecture with Coding Agents

#### Universal Method: OS-Level Input Simulation
Works with **any** application. Murmur types characters into the focused window via `enigo`.
- **Pros:** Zero integration work per agent, universal
- **Cons:** Focus-dependent, can't access agent context, timing sensitivity

#### Cursor / VS Code: Extension API
A lightweight VS Code extension that communicates with the Murmur core process via local WebSocket.
- Extension receives transcribed text and inserts it at cursor position
- Extension provides workspace context (open file, project root) back to Murmur for directory mapping
- Can target specific UI elements (chat panel, editor, terminal)

#### Claude Code: Multiple Strategies
1. **stdin piping:** `murmur --stream | claude` -- pipe continuous transcription to Claude Code's stdin
2. **MCP Server:** Murmur runs as an MCP (Model Context Protocol) server. Claude Code connects to it and can request voice input as a tool.
3. **OS-level simulation** as fallback

#### Gemini / GLM / Other Agents
- **Gemini Live API:** Direct audio streaming for real-time voice interaction (optional cloud mode)
- **HTTP/WebSocket:** Murmur exposes a local API that any agent can connect to
- **Clipboard + paste simulation** as universal fallback

```
Integration Priority:
1. OS-level input simulation (works everywhere, MVP)
2. CLI stdin/stdout piping (Claude Code, terminal agents)
3. VS Code Extension (Cursor, VS Code)
4. MCP Server (Claude Code structured integration)
5. Local WebSocket API (any agent)
6. Gemini Live API (optional cloud)
```

### 5.5 Directory Mapping System

This is a key differentiator. The system works in three layers:

**Layer 1 - Project Indexing:**
On activation (or when the working directory changes), Murmur walks the project directory tree and builds an in-memory index of all files and directories. It uses `notify` (file watcher) to keep this index updated. `.gitignore` patterns are respected.

**Layer 2 - Alias Resolution:**
A configurable alias table maps common spoken forms to path segments:
```toml
[aliases]
"source" = "src"
"components" = "src/components"
"node modules" = "node_modules"
"package json" = "package.json"
"public" = "public"
"dot env" = ".env"
"tests" = "__tests__"
```

**Layer 3 - Fuzzy + Phonetic Matching:**
When a user says "the source components header file", the system:
1. Tokenizes: ["source", "components", "header", "file"]
2. Resolves aliases: ["src", "src/components", "header", "file"]
3. Walks the index: `src/` -> `src/components/` -> fuzzy match "header" against children
4. Returns best match: `src/components/Header.tsx`
5. If ambiguous, presents top 3 candidates via the UI

Phonetic matching (Soundex/Metaphone) handles pronunciation variations.

### 5.6 Custom Vocabulary from Codebase

Murmur uses tree-sitter to parse source files and extract:
- Function/method names
- Class/struct names
- Variable names (exported/public)
- Import paths
- Package names from package.json, Cargo.toml, etc.

These are added to a "hot words" list that biases the STT engine. whisper.cpp supports `initial_prompt` for vocabulary biasing. This means if your project has a function called `calculateTotalRevenue`, the STT engine is biased to produce that exact string rather than "calculate total revenue" as separate words.

---

## 6.0 Security & Privacy

### 6.1 Principles
1. **Local-first by default.** All audio processing, STT inference, and text processing happen on-device. Zero network calls.
2. **No telemetry by default.** Optional, anonymous usage analytics require explicit opt-in.
3. **No audio storage by default.** Audio buffers are discarded after transcription. Optional recording requires explicit opt-in.
4. **Open source.** Full source code available for community audit.

### 6.2 Threat Model
| Threat | Mitigation |
|--------|-----------|
| Audio data exfiltration | No network calls in default mode. Network access requires user-enabled cloud mode. |
| Model supply chain attack | Verify model checksums on download. Support airgapped model installation. |
| Keystroke injection | Validate transcription output against character whitelist before input simulation. |
| Config file tampering | Store config in user-protected directories with standard OS permissions. |
| Memory-resident audio | Zero audio buffers immediately after STT processing. No swap-to-disk of audio data. |

### 6.3 Licensing
All dependencies must be compatible with **MIT or Apache 2.0**. No GPL dependencies in the core binary (to allow commercial use and proprietary extensions).

---

## 7.0 Implementation Phases & Epics

### Phase 1: Foundation (Weeks 1-6)
**Goal:** Working push-to-talk with accurate transcription, inserted into any app.

#### Epic 1.1: Audio Pipeline
- Set up Rust project with Cargo workspace
- Integrate CPAL for cross-platform microphone capture (macOS CoreAudio, Windows WASAPI)
- Implement audio ring buffer (16kHz, 16-bit mono PCM)
- Integrate Silero VAD via ONNX Runtime for speech detection
- Implement push-to-talk state machine (hotkey down -> record, hotkey up -> process)
- **Deliverable:** Audio captured from mic, silence trimmed, PCM buffer ready for STT

#### Epic 1.2: STT Integration
- Integrate whisper-rs (Rust bindings for whisper.cpp)
- Implement model loader (download + verify checksum on first run)
- Support model selection: base.en, small.en, medium.en
- Implement transcription pipeline: PCM buffer -> whisper -> raw text
- Add GPU acceleration detection (Metal on macOS, CUDA on Windows)
- **Deliverable:** Spoken audio -> accurate English text

#### Epic 1.3: Text Output
- Integrate enigo for cross-platform keystroke simulation
- Integrate arboard for clipboard operations
- Implement output strategies: type-character-by-character, clipboard-paste, stdout
- Register global hotkey (configurable, default: Ctrl+Shift+Space / Cmd+Shift+Space)
- **Deliverable:** Speak -> text appears in any focused application

#### Epic 1.4: CLI Interface
- Build CLI with clap: `murmur listen`, `murmur config`, `murmur models`
- Implement `--stdout` mode for piping to other tools
- Implement `--clipboard` mode for clipboard output
- Add model management commands (download, list, select)
- **Deliverable:** `murmur listen --stdout | claude` works

#### Epic 1.5: Basic Config
- TOML config file (~/.murmur/config.toml)
- Configurable: hotkey, model, output mode, audio device
- **Deliverable:** User can customize basic settings

**Phase 1 Exit Criteria:**
- [x] Push-to-talk works on macOS and Windows
- [x] WER <10% on English technical speech (base.en model)
- [x] Latency <500ms end-to-end
- [x] Text appears in any application via keystroke simulation
- [x] CLI piping works with Claude Code

---

### Phase 2: Code Intelligence (Weeks 7-12)
**Goal:** Code-aware transcription with technical jargon accuracy and voice commands.

#### Epic 2.1: Voice Commands
- Implement command parser (regex + keyword matching)
- Core commands: "new line", "new paragraph", "period", "comma", "question mark", "exclamation point"
- Formatting commands: "code block", "backtick", "backticks", "bullet point", "numbered list"
- Navigation commands: "select all", "undo", "copy that", "paste"
- Implement command vs. dictation disambiguation
- **Deliverable:** "create a function that... new line... takes two parameters" works

#### Epic 2.2: Custom Vocabulary
- Implement developer dictionary (500+ common programming terms)
- Support user-defined terms in config (~/.murmur/vocabulary.json)
- Implement whisper.cpp prompt biasing with vocabulary terms
- Add symbol expansion: "arrow function" -> "=>", "triple equals" -> "==="
- **Deliverable:** "async await" never becomes "a sink a weight"

#### Epic 2.3: Project Indexer
- Implement recursive directory walker (respects .gitignore)
- Build file/directory trie index
- Integrate tree-sitter for parsing JS/TS/Python/Rust/Go files
- Extract identifiers (functions, classes, variables) from parsed AST
- Integrate notify for filesystem watching (live index updates)
- Merge extracted identifiers into hot words list
- **Deliverable:** STT engine knows about your project's functions and classes

#### Epic 2.4: Modes
- Implement mode system: coding, prose, command
- Coding mode: tech vocabulary bias, no auto-capitalization, symbol shortcuts
- Prose mode: natural punctuation, capitalization, filler word removal
- Command mode: short utterance optimization, high-confidence thresholds
- Mode switching: voice command ("switch to coding mode") or hotkey
- **Deliverable:** Different behavior for "write me an email" vs "create a React component"

#### Epic 2.5: System Tray UI (Tauri)
- Minimal system tray application (Tauri v2)
- Show recording status (idle, listening, processing)
- Quick settings: model, mode, hotkey, audio device
- Audio level meter
- Transcription history (last 20 entries)
- **Deliverable:** Polished tray app users can interact with

**Phase 2 Exit Criteria:** (status as of v0.3.6)
- [x] Voice commands work reliably
- [~] Custom vocabulary supported as a manual user list; codebase auto-extraction (tree-sitter) not built
- [ ] Project file index built and kept up to date (not built)
- [~] Transcription profile switching exists; full coding/prose/command modes not built
- [x] System tray UI functional on Windows (macOS not yet shipped)

---

### Phase 3: Deep Integration (Weeks 13-18)
**Goal:** Native integrations with coding agents and directory mapping.

#### Epic 3.1: Directory Mapping
- Implement alias resolution table (config-driven)
- Implement fuzzy matching against project file index
- Implement phonetic matching (Soundex/Metaphone) for path segments
- Add disambiguation UI (show top 3 candidates if unclear)
- Detect directory context: "the header component" -> path, "header component" -> just text
- **Deliverable:** "go to source components header" -> `src/components/Header.tsx`

#### Epic 3.2: VS Code / Cursor Extension
- TypeScript extension that connects to Murmur core via local WebSocket
- Provides workspace context to Murmur (project root, open files, active file)
- Inserts text at cursor position in editor
- Targets Cursor AI chat panel when appropriate
- Status bar indicator showing Murmur connection state
- **Deliverable:** Voice-to-Cursor with full project awareness

#### Epic 3.3: Claude Code MCP Integration
- Implement MCP server in Murmur (stdio transport)
- Expose tools: `voice_listen` (start recording), `voice_transcribe` (return text)
- Expose resources: `project_files` (file index), `voice_history` (recent transcriptions)
- Claude Code config: `claude mcp add murmur -- murmur mcp-server`
- **Deliverable:** Claude Code can request voice input as a tool

#### Epic 3.4: Advanced Code Formatting
- Implement case conversion commands: "camel case get user data" -> `getUserData`
- Implement "spell mode" for letter-by-letter input of unusual identifiers
- Implement bracket/parenthesis pairing: "open paren... close paren"
- Context-aware formatting: detect if target is a code editor vs. chat
- **Deliverable:** Natural voice-to-code formatting

#### Epic 3.5: Local WebSocket API
- Murmur exposes `ws://localhost:PORT/murmur` for any agent to connect
- JSON message protocol: `{type: "transcription", text: "...", mode: "coding"}`
- Enable integration with Gemini, GLM, or any tool that can open a WebSocket
- **Deliverable:** Universal integration point for any AI agent

**Phase 3 Exit Criteria:** (not started; deprioritized in favor of the dictation-quality and live-preview work that shipped instead)
- [ ] "Navigate to source components header" resolves correctly (directory mapping not built)
- [ ] VS Code/Cursor extension installed and working (not built)
- [ ] Claude Code MCP server functional (not built)
- [ ] WebSocket API documented and tested (not built)
- [ ] Case conversion commands work (not built)

---

### Phase 4: Polish & Performance (Weeks 19-24)
**Goal:** Production-quality performance, UX polish, and community launch.

#### Epic 4.1: Performance Optimization
- Implement streaming transcription (process while still speaking)
- Profile and optimize hot path (audio -> VAD -> STT -> output)
- Add GPU acceleration: Metal (macOS), CUDA (Windows), Vulkan (both)
- Implement model quantization (INT8/INT4) for smaller footprint
- Target: <200ms latency on Apple Silicon, <300ms on modern x86
- **Deliverable:** Noticeably faster transcription

#### Epic 4.2: Advanced VAD
- Implement "continuous mode" (always listening, auto-segment by pauses)
- Configurable silence threshold and segment duration
- Wake word support ("Hey Murmur" -> start recording)
- Background noise adaptation
- **Deliverable:** Hands-free continuous dictation option

#### Epic 4.3: Multi-Language Support
- Enable Whisper multilingual models
- Language auto-detection
- Mixed-language support (English + code in any spoken language)
- **Deliverable:** Non-English developers can use Murmur

#### Epic 4.4: Installer & Distribution
- macOS: .dmg with code signing and notarization
- Windows: .msi installer with optional portable .exe
- Auto-update mechanism (check on launch, user-approved)
- First-run setup wizard (select model, configure hotkey, test microphone)
- **Deliverable:** One-click install experience

#### Epic 4.5: Documentation & Community
- User documentation (setup, configuration, voice commands reference)
- Developer documentation (architecture, contributing guide, plugin API)
- Example integrations (Claude Code, Cursor, terminal workflow)
- GitHub repository setup with CI/CD (cross-platform builds)
- **Deliverable:** Ready for open-source launch

**Phase 4 Exit Criteria:** (status as of v0.3.6)
- [~] Streaming transcription and GPU acceleration shipped (CUDA on Windows); Apple Silicon latency not yet measured or shipped
- [~] Installer and auto-update shipped for Windows; macOS installer pending signing/notarization
- [~] Documentation in progress (README and release notes; full user/developer docs partial)
- [x] GitHub repo public with release CI/CD (Windows)

---

## 8.0 Open-Source Libraries Summary

### Core Dependencies (All Permissively Licensed)

> **Status note (v0.3.6):** The table below is the originally planned dependency set. Not yet integrated, because the features they support have not been built: `tree-sitter` and its grammars, `notify` (file watching), `strsim` (fuzzy matching), and `tungstenite` (WebSocket API). Now in use but absent from the original list: `tauri-plugin-updater` (signed auto-update) and `unicode-segmentation` (grapheme-correct editing commands). Speech-to-text also includes NVIDIA Parakeet (ONNX) alongside whisper-rs.

| Library | Version | License | Purpose |
|---------|---------|---------|---------|
| `whisper-rs` | latest | MIT | Rust bindings for whisper.cpp STT engine |
| `cpal` | 0.15+ | Apache 2.0 | Cross-platform audio capture |
| `ort` (ONNX Runtime) | latest | MIT | Run Silero VAD model |
| `silero-vad` | v5 | MIT | Voice activity detection model |
| `enigo` | 0.2+ | MIT | Cross-platform input simulation |
| `arboard` | 3.x | Apache 2.0/MIT | Cross-platform clipboard |
| `global-hotkey` | 0.6+ | Apache 2.0/MIT | System-wide hotkey registration |
| `tree-sitter` | latest | MIT | Code parsing for identifier extraction |
| `tree-sitter-*` | latest | MIT | Language grammars (JS, TS, Python, Rust, Go, etc.) |
| `notify` | 7.x | CC0/Artistic 2.0 | File system watching |
| `strsim` | 0.11+ | MIT | Fuzzy string matching |
| `clap` | 4.x | Apache 2.0/MIT | CLI argument parsing |
| `tokio` | 1.x | MIT | Async runtime |
| `serde` + `toml` | latest | MIT | Config serialization |
| `tauri` | 2.x | Apache 2.0/MIT | Desktop UI framework |
| `tungstenite` | latest | MIT/Apache 2.0 | WebSocket server |
| `tracing` | latest | MIT | Structured logging |

### STT Models (Downloaded at Runtime)

| Model | Size | License | Source |
|-------|------|---------|--------|
| whisper base.en | 142MB | MIT | OpenAI / ggml |
| whisper small.en (default) | 466MB | MIT | OpenAI / ggml |
| whisper medium.en | 1.5GB | MIT | OpenAI / ggml |
| whisper large-v3-turbo (multilingual + translate) | 1.6GB | MIT | OpenAI / ggml |
| Parakeet TDT 0.6B v2 (ONNX) | ~660MB | CC-BY-4.0 model weights | NVIDIA / istupakov ONNX export |

---

## 9.0 Success Metrics

| Metric | Phase 1 Target | Phase 4 Target |
|--------|---------------|----------------|
| **WER (English, clean)** | <10% | <7% |
| **End-to-end latency** | <500ms | <200ms (Apple Silicon) |
| **Memory (idle)** | <150MB | <100MB |
| **Memory (active, small model)** | <600MB | <500MB |
| **Cold start time** | <5s | <3s |
| **Command recognition accuracy** | -- | >95% |
| **Directory resolution accuracy** | -- | >85% (top-3) |
| **Platforms** | macOS + Windows | macOS + Windows + Linux |

---

## 10.0 Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| whisper.cpp accuracy insufficient for code jargon | Medium | High | Custom vocabulary biasing + post-processing pipeline. Fallback to larger models. Evaluate Parakeet TDT as alternative. |
| Cross-platform audio inconsistencies | Medium | Medium | CPAL abstracts OS differences. Extensive testing matrix. Platform-specific fallbacks. |
| Input simulation blocked by OS security (macOS accessibility permissions) | High | Medium | Clear first-run permission request flow. Documentation for granting accessibility access. |
| Model download size deters users | Medium | Low | Default to base.en (142MB). Progressive model download. Clear size/accuracy tradeoffs in UI. |
| Latency too high on CPU-only machines | Medium | High | Default to smallest model. Streaming transcription. Quantized models. |
| tree-sitter parsing too slow on large codebases | Low | Low | Incremental parsing. Limit index to top-level identifiers. Lazy loading. |

---

## 11.0 Future Considerations (Post-v1)

- **Speaker identification:** Multi-user environments (pair programming)
- **Text-to-speech:** Read back AI agent responses
- **Gemini Live API integration:** Direct audio-to-agent pipeline
- **Custom model fine-tuning:** Train on user's voice and vocabulary
- **Plugin system:** Community-built integrations and formatters
- **Mobile companion:** iOS/Android app for voice-to-code on the go
- **Team vocabularies:** Shared project dictionaries via git
