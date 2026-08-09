# Configuration reference

Every Murmur setting lives in one TOML file, `config.toml`, inside the
`murmur` folder of your system config directory: `%APPDATA%\murmur` on
Windows, `~/.config/murmur` on Linux, and `~/Library/Application Support/murmur`
on macOS. The Settings view edits this file for you, but you can also edit it
by hand while the app is closed. Saves are atomic, and if the file is ever
corrupt Murmur backs it up to `config.toml.bak` and restarts from defaults
rather than failing to launch. A config directory from the app's old name,
`voitex`, is migrated automatically on first run.

## Recording and activation

`hotkey` is the global push-to-talk shortcut, `ctrl+shift+space` by default
(`super+shift+space` on macOS). `double_tap_key` is the key that toggles
recording when tapped twice quickly; the default is `rctrl` on Windows and
`ctrl` elsewhere, and it can also be a single letter like `v`.
`activation_mode` chooses how that key behaves: `toggle` (the default, tap
twice to start and twice to stop) or `hold` (record while the key is held).
`click_to_stop` (off by default) stops recording on any mouse click instead of
only the hotkey or mic button.

## Voice detection and timing

`vad_threshold` is the speech probability needed to count audio as speech,
0.3 by default, valid from 0.05 to 0.95; higher values reject quieter speech.
`silence_rms_threshold` is the loudness floor below which audio counts as
silence, from 0.0 to 1.0; the default 0.0 auto-calibrates from ambient noise.
`silence_timeout_secs` (default 2.5) is how long silence must last after
speech before recording auto-stops. `phrase_pause_secs` (default 0.6, valid
0.3 to 10) is the pause that ends one phrase and delivers it during
streaming. `session_timeout_secs` (default 60, up to 300) ends a session
after that much total inactivity; 0 disables the timeout.

## Model and language

`model` selects the STT model; the default is `parakeet-tdt-06b-v2`. Whisper
variants are `whisper-base-en`, `whisper-small-en`, `whisper-medium-en`, and
`whisper-large-v3-turbo`, plus the multilingual `parakeet-tdt-06b-v3`.
`language` is the spoken language code, `en` by default, or `auto` to detect;
only Whisper Large v3 Turbo honors it, while Parakeet v3 always auto-detects
and the English-only models ignore it. `translate_to_english` (off) makes
Whisper Large v3 Turbo translate speech to English, and
`show_translated_caption` (off) shows that translation in the live caption.
`transcription_profile` is `relaxed` (the default, permissive with quiet or
short phrases) or `strict` (stronger hallucination filtering).
`model_idle_unload_secs` frees the model's RAM after that many idle seconds
and reloads it on next use; 0 (the default) keeps it loaded forever, and
non-zero values must be between 60 and 86400.

## Output and delivery

`output_mode` is how text reaches the target app: `auto` (the default: try
keyboard, then clipboard paste, then clipboard only), `keyboard`,
`clipboard_paste`, `clipboard`, or `stdout` (for CLI piping).
`pre_output_delay_ms` (default 80) waits that long after hotkey release
before typing so the target window can regain focus; 0 disables the wait.
`developer_mode` (off) turns on code-oriented post-processing.
`clean_speech` (on) strips "um" and "uh" and formats spoken number lists in
ordinary dictation. `smart_punctuation` (on) repairs a sentence that a pause
split in two, removing the stale period and joining the phrases.
`default_rewrite_mode` applies a local LLM rewrite to delivered text when no
app profile overrides it: `clean_up`, `formal`, `casual`, `bullet_list`, or
`summarize`; leave it unset for no rewrite.

## Interface and feedback

`show_widget` (on) shows the floating pill. `sound_feedback` (on) plays a
short chime when recording starts and stops. `live_preview` (on) shows
interim transcription while you speak; turning it off gives the lowest
latency. `caption_position` places that live caption: `pill` (the default,
under the floating pill) or `window` (near the bottom of the active window).
`daily_word_goal` (0 = disabled, up to 100000) shows a progress target on the
Analytics dashboard.

## Audio input

`audio_device` names the preferred input device; leave it unset for the
system default. `echo_cancellation` (on) uses the OS voice-capture path with
echo cancellation and noise suppression so the mic ignores your speakers;
Windows only for now, elsewhere the raw mic is used. `mic_warm_start` (off)
keeps the mic stream open between dictations so the first word is never
clipped; while idle the audio is discarded immediately, but the OS
mic-in-use indicator stays lit, which is why it is opt-in.

## Vocabulary, snippets, and profiles

`custom_vocabulary` lists words the model tends to get wrong (up to 100
entries). `snippets` holds trigger and expansion pairs (up to 100).
`clipboard_placeholders` are spoken phrases replaced inline by your clipboard
text; the defaults are "insert clipboard" and "paste clipboard", up to 16
entries, and an empty list disables the feature. `app_profiles` holds per-app
overrides (up to 50); see the Per-app profiles article for the fields.
`path_aliases` maps spoken phrases to path segments for command mode
navigation (up to 100 entries). Lists over their cap are trimmed on load, not
rejected.

## Codebase vocabulary

The `[indexer]` table controls project symbol extraction. `enabled` is off by
default. `project_roots` lists the folders to scan; indexing only runs when
at least one is set. `max_symbols` caps how many symbols are injected,
64 by default and clamped between 1 and 128. `extensions` limits which source
extensions are scanned; empty means the built-in defaults.

## Privacy and integrations

`save_history` (on) persists delivered phrases to the local searchable
history; turn it off to store nothing. `mcp_dictation_enabled` (on) lets a
connected coding agent request voice capture through MCP; off keeps the MCP
server strictly read-only. `local_api_enabled` (off) exposes the
localhost-only WebSocket API for editor plugins; it is off by default because
it opens a network listener, even a loopback-only one.
`context_injection_enabled` (off) adds the target app's name and your
clipboard text to selection-rewrite prompts, strictly on-device. The
`[cloud]` table is the opt-in bring-your-own-key cloud rewrite backend, with
`enabled` (false by default), `base_url`, and `model`; the API key is never
stored in the file and is read from the `MURMUR_CLOUD_API_KEY` environment
variable instead. With no `[cloud]` table, or `enabled = false`, Murmur makes
no network calls.

## Settings only in the file

Most settings appear in the Settings view, but a couple are file-only:
`clipboard_placeholders` and `silence_rms_threshold` are edited here when you
need them. If a value you
enter is out of range, Murmur rejects the file on load, backs it up, and
starts from defaults, so keep a copy of a heavily customized config.
