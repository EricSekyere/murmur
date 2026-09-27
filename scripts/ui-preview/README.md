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

The top toolbar has a page switcher with arrow-key navigation, Home/End,
Enter, and Escape. Use `?platform=macos` or `?platform=linux` to review the
platform layouts. Native traffic lights are supplied by macOS, so they do not
appear in the browser preview. Window buttons are inert in the browser; the
single application-menu button opens the same styled parent dropdown and
side submenus as the desktop app, with sample command state. The parent also
provides access to the full native fallback. Edit and macOS system menus use a browser stand-in
for the native fallback. All five Windows/Linux menu groups remain available;
macOS retains its application and Window groups as well.
Page navigation, About, the microphone, and the pill use sample data; commands
that require the desktop app explain this when selected.

With Playwright available, run `node scripts/ui-preview/check-chrome.cjs` from
the repository root while the preview server is running. Set `PLAYWRIGHT_MODULE`
to an installed Playwright module path and `CHROMIUM_PATH` to its browser binary
when they are not in their default locations. The check covers 90 combinations
of platform layout, theme, view, and window size, plus keyboard behavior and
preservation of all original controls against `main`. Native window operations
are mocked: this does not replace OS-level tests. Reports and screenshots go
under `target/`. Set `MURMUR_PREVIEW_URL` for a nondefault preview port.

Run `node scripts/ui-preview/check-menus.cjs` with the same environment to compare
all 24 original Windows commands with `menu-baseline.json`, and check the styled
menu's keyboard navigation, disabled states, platform groups, native fallback,
and recovery when the native menus finish initializing after the frontend.
The baseline contains only command labels captured from the installed release.
