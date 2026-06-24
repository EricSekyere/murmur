# Developer mode and code dictation

Developer mode post-processes your transcription for programming: it corrects
tech terms, expands spoken symbols, removes fillers, and adds casing commands.
Turn it on in Settings under Developer Mode. A DEV badge shows when it is active.

## Tech term correction

Developer mode fixes the capitalization and spelling of common technical terms,
so "typescript" becomes TypeScript, "github" becomes GitHub, "api" becomes API,
and "use state" becomes useState. It covers languages, frameworks, React hooks,
acronyms, databases, cloud tools, and AI tools.

## Spoken symbol expansion

Spoken operators and punctuation become real symbols. Say "fat arrow" for =>,
"triple equals" for ===, "open paren" and "close paren" for parentheses, "double
ampersand" for &&, "semicolon", "backtick", and many more. Multi-word symbols are
matched first, so longer phrases win over shorter ones.

## Casing commands

Casing keywords reformat the words that follow them: "camel get user name"
becomes getUserName, "snake" gives snake_case, "pascal" gives PascalCase, "kebab"
gives kebab-case, and "upper" gives UPPER_SNAKE. The keyword collects words until
the next casing keyword, a stop word (like "and" or "the"), or the end.

## Filler removal

Developer mode strips hesitations like "um" and "uh", collapses stuttered
function words ("the the" to "the"), and drops fillers such as "you know",
"basically", "actually", and "literally". The result is cleaner code-oriented
text without you trailing off.

## Clean up speech for prose

Outside developer mode, the Clean up speech setting does a lighter pass on
ordinary dictation: it removes "um" and "uh" disfluencies and formats spoken
number lists, while leaving meaningful words alone. Turn it off for fully verbatim
text. Developer mode always runs its own fuller cleanup regardless of this toggle.

## Spoken number lists

When you dictate "number one ... number two ..." with at least two sequential
markers, Murmur turns it into a numbered list. This is deliberately conservative
and only fires on the explicit "number N" form, so mentioning a number in normal
prose is never reshaped.

## Per-app developer mode

You do not have to toggle developer mode by hand for each app. App Profiles can
switch it on automatically for your editor and off for chat, based on the focused
window. See the per-app profiles help for details.
