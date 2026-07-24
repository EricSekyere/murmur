# Output and delivery

Output mode controls how your transcribed text reaches the focused app. Set it in
Settings under Output Mode. The default, Auto, works well almost everywhere.

## Auto mode

Auto types the characters directly into the focused window using simulated
keystrokes, and falls back to clipboard and paste only if direct typing fails
(for example in an elevated window). Direct typing never touches your clipboard,
so your previous clipboard contents can never leak into the target. This is the
recommended mode.

## Keyboard mode

Keyboard mode always types the characters as keystrokes and never routes through
the clipboard. It behaves like Auto for the common case. Pick it if you want to
be sure Murmur never copies anything to your clipboard.

## Clipboard and paste mode

Clipboard + Paste copies the text and simulates Ctrl+V (Cmd+V on macOS) to paste
it. This can be more reliable than typing in some terminals and elevated windows.
The tradeoff is that it briefly uses your clipboard.

## Clipboard only mode

Clipboard Only copies the text and stops there, so you paste it yourself whenever
you are ready. Nothing is typed or pasted automatically.

## Window targeting

Text goes to whatever window was focused when the session started, not whatever
is focused when transcription finishes. This means you can glance at the pill or
another window mid sentence without misdelivering your words. An App Profile
with the submit option can also press Enter or Ctrl+Enter for you once a
session ends, sending the dictated message hands-free.

## Smart punctuation

Speech models close a phrase with a full stop or question mark the moment you
pause, even when your sentence was not finished. With Smart punctuation on
(the default), Murmur repairs the junction when you continue: "went to the
store. And bought" becomes "went to the store and bought". The repair happens
only when you continue within a few seconds in the same window Murmur was
typing into, so text you finished, moved away from, or touched by hand is
never rewritten. Turn it off in Settings for strictly verbatim delivery.

## Terminals

Modern terminals accept direct Unicode typing fine, so Auto usually just works.
If a particular terminal or elevated app drops characters, set Clipboard + Paste
for it, optionally only for that app using App Profiles.

## Wrong window fallback

If direct typing cannot reach the target, Murmur automatically tries clipboard
and paste, and if that also fails it copies the text to the clipboard so nothing
is lost. On Linux under Wayland, direct typing into other apps is blocked by the
system, so Murmur falls back to clipboard and paste there.
