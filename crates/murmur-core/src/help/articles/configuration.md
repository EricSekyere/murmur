# Configuration reference

Every Murmur setting lives in one TOML file, `config.toml`, inside the
`murmur` folder of your system config directory: `%APPDATA%\murmur` on
Windows, `~/.config/murmur` on Linux, and `~/Library/Application Support/murmur`
on macOS. The Settings view edits this file for you, but you can also edit
it by hand while the app is closed.

> **Note:** Saves are atomic, and if the file is ever corrupt Murmur backs
> it up to `config.toml.bak` and restarts from defaults rather than failing
> to launch. A config directory from the app's old name, `voitex`, is
> migrated automatically on first run.

## Recording and activation

| Setting | Default | What it does |
| --- | --- | --- |
| `hotkey` | `ctrl+shift+space` (`super+shift+space` on macOS) | Global push-to-talk shortcut |
| `double_tap_key` | `rctrl` on Windows, `ctrl` elsewhere | Key that toggles recording when tapped twice quickly; can also be a single letter like `v` |
| `activation_mode` | `toggle` | `toggle` (tap twice to start and twice to stop) or `hold` (record while held) |
| `click_to_stop` | off | Stops recording on any mouse click instead of only the hotkey or mic button |

## Voice detection and timing

| Setting | Default | Range | What it does |
| --- | --- | --- | --- |
| `vad_threshold` | 0.3 | 0.05 to 0.95 | Speech probability needed to count audio as speech; higher rejects quieter speech |
| `silence_rms_threshold` | 0.0 | 0.0 to 1.0 | Loudness floor below which audio counts as silence; 0.0 auto-calibrates from ambient noise |
| `silence_timeout_secs` | 2.5 | above 0 | Silence after speech before recording auto-stops |
| `phrase_pause_secs` | 0.6 | 0.3 to 10 | Pause that ends one phrase and delivers it during streaming |
| `session_timeout_secs` | 60 | 0 to 300 | Total inactivity that ends a session; 0 disables the timeout |

## Model and language

| Setting | Default | What it does |
| --- | --- | --- |
| `model` | `parakeet-tdt-06b-v2` | STT model; Whisper variants are `whisper-base-en`, `whisper-small-en`, `whisper-medium-en`, `whisper-large-v3-turbo`, plus the multilingual `parakeet-tdt-06b-v3` |
| `language` | `en` | Spoken language code, or `auto` to detect; only Whisper Large v3 Turbo honors it (Parakeet v3 always auto-detects, English-only models ignore it) |
| `translate_to_english` | off | Makes Whisper Large v3 Turbo translate speech to English |
| `show_translated_caption` | off | Shows that translation in the live caption |
| `transcription_profile` | `relaxed` | `relaxed` is permissive with quiet or short phrases; `strict` filters hallucinations harder |
| `model_idle_unload_secs` | 0 | Frees the model's RAM after that many idle seconds and reloads on next use; 0 keeps it loaded forever, non-zero values must be 60 to 86400 |

## Output and delivery

| Setting | Default | What it does |
| --- | --- | --- |
| `output_mode` | `auto` | `auto` (try keyboard, then clipboard paste, then clipboard only), `keyboard`, `clipboard_paste`, `clipboard`, or `stdout` for CLI piping |
| `pre_output_delay_ms` | 80 | Wait after hotkey release before typing so the target window regains focus; 0 disables |
| `developer_mode` | off | Code-oriented post-processing |
| `clean_speech` | on | Strips "um" and "uh" and formats spoken number lists in ordinary dictation |
| `smart_punctuation` | on | Repairs a sentence that a pause split in two, removing the stale period and joining the phrases |
| `default_rewrite_mode` | unset | Local LLM rewrite applied when no app profile overrides it: `clean_up`, `formal`, `casual`, `bullet_list`, or `summarize` |

## Interface and feedback

| Setting | Default | What it does |
| --- | --- | --- |
| `show_widget` | on | Shows the floating pill |
| `sound_feedback` | on | Plays a short chime when recording starts and stops |
| `live_preview` | on | Shows interim transcription while you speak; off gives the lowest latency |
| `caption_position` | `pill` | Places the live caption: `pill` (under the floating pill) or `window` (near the bottom of the active window) |
| `daily_word_goal` | 0 (disabled) | Progress target on the Analytics dashboard, up to 100000 |

## Audio input

| Setting | Default | What it does |
| --- | --- | --- |
| `audio_device` | unset (system default) | Names the preferred input device |
| `echo_cancellation` | on | OS voice-capture path with echo cancellation and noise suppression so the mic ignores your speakers; Windows only, elsewhere the raw mic is used |
| `mic_warm_start` | off | Keeps the mic stream open between dictations so the first word is never clipped; idle audio is discarded immediately, but the OS mic-in-use indicator stays lit, which is why it is opt-in |

## Vocabulary, snippets, and profiles

| Setting | Cap | What it holds |
| --- | --- | --- |
| `custom_vocabulary` | 100 entries | Words the model tends to get wrong |
| `snippets` | 100 entries | Trigger and expansion pairs |
| `clipboard_placeholders` | 16 entries | Spoken phrases replaced inline by your clipboard text; defaults are "insert clipboard" and "paste clipboard", an empty list disables the feature |
| `app_profiles` | 50 entries | Per-app overrides; see the Per-app profiles article for the fields |
| `path_aliases` | 100 entries | Spoken phrase to path segment mappings for command mode navigation |

Lists over their cap are trimmed on load, not rejected.

## Codebase vocabulary

The `[indexer]` table controls project symbol extraction. `enabled` is off
by default. `project_roots` lists the folders to scan; indexing only runs
when at least one is set. `max_symbols` caps how many symbols are
injected, 64 by default and clamped between 1 and 128. `extensions`
limits which source extensions are scanned; empty means the built-in
defaults.

## Privacy and integrations

| Setting | Default | What it does |
| --- | --- | --- |
| `save_history` | on | Persists delivered phrases to the local searchable history; off stores nothing |
| `mcp_dictation_enabled` | on | Lets a connected coding agent request voice capture through MCP; off keeps the MCP server strictly read-only |
| `local_api_enabled` | off | Exposes the localhost-only WebSocket API for editor plugins; off by default because it opens a network listener, even a loopback-only one |
| `context_injection_enabled` | off | Adds the target app's name and your clipboard text to selection-rewrite prompts, strictly on-device |

The `[cloud]` table is the opt-in bring-your-own-key cloud rewrite
backend, with `enabled` (false by default), `base_url`, and `model`. The
API key is never stored in the file; it is read from the
`MURMUR_CLOUD_API_KEY` environment variable instead.

> **Privacy:** With no `[cloud]` table, or `enabled = false`, Murmur makes
> no network calls.

## Relocating the config directory

For testing, portable setups, or scripted runs, three environment variables
move where Murmur looks for its files. Most users never need these.

| Variable | What it overrides |
| --- | --- |
| `MURMUR_CONFIG_DIR` | The system config base; Murmur then uses `<value>/murmur` for its config, history, and related files |
| `MURMUR_DATA_DIR` | The system data base; Murmur then uses `<value>/murmur` for downloaded models, the ONNX Runtime, and other caches |
| `MURMUR_HOME_DIR` | The home directory used when writing editor MCP configs such as `~/.cursor/mcp.json` |

> **Note:** Only absolute paths are honored; an empty or relative value is
> ignored and the platform default is used. If the override points somewhere
> unwritable, Murmur still starts and simply runs with defaults it cannot
> persist.

## Settings only in the file

Most settings appear in the Settings view, but a couple are file-only:
`clipboard_placeholders` and `silence_rms_threshold` are edited here when
you need them.

> **Warning:** If a value you enter is out of range, Murmur rejects the
> file on load, backs it up, and starts from defaults, so keep a copy of
> a heavily customized config.
