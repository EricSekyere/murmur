# UI design preview

Run from the repository root:

```powershell
node scripts/ui-preview.cjs
```

Open http://127.0.0.1:4173. Set `MURMUR_PREVIEW_PORT` to use a different port.
Stop the server with Ctrl+C.

The server serves the actual desktop frontend and injects a separate, in-memory
Tauri bridge. It binds only to loopback. History, models, activity, and meetings
are sample data. Help uses the repository's bundled articles. The browser never
reads or writes the installed application's data, uses the microphone, downloads
models, runs commands, or installs updates. Unsupported native actions explain
that the desktop app is required.

Settings edits last until the page reloads. Appearance is saved in this browser's
local storage, just as it is in the app's own webview. The two stores are separate.
Use **Review setup** to inspect onboarding. **Find pill** shows the unchanged
floating widget in a browser frame; it does not locate the installed widget.

Review Dictate (including history, rewrite, file transcription, and Meetings),
Analytics, all nine Settings sections, Diagnostics, Help articles, and dialogs.
The sidebar appearance switch and Settings > Behavior & interface > Appearance
both control the light/dark preference. Dark remains the default.
