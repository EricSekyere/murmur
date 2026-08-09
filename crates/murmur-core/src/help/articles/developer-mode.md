# Developer mode and code dictation

Developer mode post-processes your transcription for programming: it
corrects tech terms, expands spoken symbols, removes fillers, and adds
casing commands. Turn it on in Settings under Developer Mode. A DEV badge
shows when it is active.

## Tech term correction

Developer mode fixes the capitalization and spelling of common technical
terms. It covers languages, frameworks, React hooks, acronyms, databases,
cloud tools, and AI tools.

| Say | Get |
| --- | --- |
| "typescript" | TypeScript |
| "github" | GitHub |
| "api" | API |
| "use state" | useState |
| "kubernetes" | Kubernetes |
| "postgres" | PostgreSQL |

## Spoken symbol expansion

Spoken operators and punctuation become real symbols.

| Say | Get |
| --- | --- |
| "fat arrow" | `=>` |
| "thin arrow" | `->` |
| "triple equals" | `===` |
| "not equals" | `!=` |
| "double ampersand" | `&&` |
| "double colon" | `::` |
| "spread operator" | `...` |
| "optional chaining" | `?.` |
| "null coalescing" | `??` |
| "plus equals" | `+=` |
| "open paren" and "close paren" | `(` and `)` |
| "open bracket" and "close bracket" | `[` and `]` |
| "open brace" and "close brace" | `{` and `}` |
| "semicolon" | `;` |
| "underscore" | `_` |

Many more work the same way: "pipe" and "double pipe", "backtick",
"ampersand", "caret", "at sign", "hash", "dollar sign", "single quote",
"double quote", "backslash", "tab", and the rest of the common punctuation.
Multi-word symbols are matched first, so longer phrases win over shorter
ones.

## Casing commands

Casing keywords reformat the words that follow them.

| Say | Get |
| --- | --- |
| "camel get user name" | `getUserName` |
| "snake get user name" | `get_user_name` |
| "pascal user service" | `UserService` |
| "kebab my component" | `my-component` |
| "upper max retries" | `MAX_RETRIES` |

The keyword collects words until the next casing keyword, a stop word (like
"and" or "the"), or the end of the phrase.

## Filler removal

Developer mode strips hesitations like "um" and "uh", collapses stuttered
function words ("the the" to "the"), and drops fillers such as "you know",
"basically", "actually", and "literally". The result is cleaner
code-oriented text without you trailing off.

## Clean up speech for prose

Outside developer mode, the Clean up speech setting does a lighter pass on
ordinary dictation: it removes "um" and "uh" disfluencies and formats spoken
number lists, while leaving meaningful words alone. Turn it off for fully
verbatim text.

> **Note:** Developer mode always runs its own fuller cleanup regardless of
> this toggle.

## Spoken number lists

When you dictate "number one ... number two ..." with at least two
sequential markers, Murmur turns it into a numbered list. This is
deliberately conservative and only fires on the explicit "number N" form, so
mentioning a number in normal prose is never reshaped.

## Per-app developer mode

You do not have to toggle developer mode by hand for each app.

> **Tip:** App Profiles can switch developer mode on automatically for your
> editor and off for chat, based on the focused window. See the per-app
> profiles help for details.
