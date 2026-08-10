# Snippets and personal dictionary

Murmur has two ways to handle words and phrases it would otherwise get
wrong: text snippets that expand a spoken trigger, and a personal
dictionary that biases spelling. Both live in Settings.

## Text snippets

A snippet maps a spoken trigger to an expansion. Enter them in Settings,
one per line, as trigger = expansion. Say the trigger as a whole phrase
and Murmur types the expansion instead.

| Say | Get |
| --- | --- |
| "my email" | Your address, exactly as you wrote the expansion |
| "sign off" | Your closing line |

## Snippets with slots

A trigger can capture part of what you say. Write a slot as {name} in
the trigger, then reference it in the expansion as {name} for the words
as spoken or {name:transform} for a recased version. A slot may appear
several times with different transforms.

| You wrote | You say | Murmur types |
| --- | --- | --- |
| new test {name} = fn {name:snake}() | "new test user login" | fn user_login() |
| import {module} = import {module:camel} from './{module:kebab}'; | "import date utils" | import dateUtils from './date-utils'; |
| my branch {name} = feature/{name:kebab} | "my branch fix login bug" | feature/fix-login-bug |

Captures arrive lowercase, because matching ignores case; the transforms
exist precisely so the expansion can restore the casing you need.

In an expansion with slots, \n inserts a line break and \t a tab, so one
line in Settings can produce a multi-line expansion. To type a literal
brace in such an expansion, double it: {{ and }}. Snippets without a
slot in the trigger are typed exactly as written, including backslashes
and braces, so existing snippets never change behavior.

## Casing transforms

All transforms shown on the same capture, "user profile card":

| Transform | Result |
| --- | --- |
| {name:pascal} | UserProfileCard |
| {name:camel} | userProfileCard |
| {name:snake} | user_profile_card |
| {name:kebab} | user-profile-card |
| {name:upper} | USER_PROFILE_CARD |
| {name:lower} | user profile card |
| {name:title} | User Profile Card |
| {name} | user profile card |

> **Note:** upper produces an UPPER_SNAKE identifier, matching the
> spoken "upper" casing formatter in developer mode.

## How snippet matching works

A snippet fires only when its trigger is the entire phrase, matched after
ignoring case, surrounding whitespace, and trailing punctuation. Saying
the trigger inside a longer sentence types the words normally. An empty
or punctuation-only trigger never fires, so silence cannot trigger an
expansion. A slot must capture at least one word, and slotted triggers
are skipped for very long phrases (over 32 words), so ordinary dictation
stays fast.

## Snippet collisions

Built-in editing commands always win over a snippet with the same
trigger, so a snippet named "scratch that" would never fire and Murmur
warns you. If two snippets share a trigger, only the first one fires and
the duplicate is flagged.

An exact trigger always beats a slotted one: with both "deploy staging"
and "deploy {env}" defined, saying "deploy staging" fires the exact
snippet. Between slotted triggers, the first one in your list that
matches wins, so put specific patterns before general ones.

> **Tip:** A trigger that is only a slot, like {x}, would swallow every
> phrase you speak, so Murmur refuses it. Keep at least one fixed word
> in the trigger. A broken slotted snippet (an unbalanced brace, a slot
> the trigger does not have) is simply disabled with a warning; the rest
> of your snippets keep working.

> **Tip:** You can still type a snippet's words literally with the
> "literally" prefix.

## Personal dictionary

The personal dictionary is a list of names, jargon, and terms the model
tends to mishear. Enter them in Settings, one per line. On Whisper models
they are injected into the decoder as a glossary that biases recognition.
On every model, including Parakeet, a correction pass then fixes close
mishearings of your terms in the finished text, so the dictionary works
no matter which engine you use.

## How dictionary corrections work

After transcription, Murmur compares each word, and short runs of two or
three words, against your dictionary. A term is corrected only when it
both sounds like and is spelled close to the dictionary entry.

| The model heard | Your dictionary fixes it to |
| --- | --- |
| "kubernetis" | Kubernetes |
| "git hub" | GitHub |

A word that already matches an entry except for casing gets just its
casing fixed. Common English words are never rewritten, and text with no
near-match of a dictionary term is left untouched.

## Learn from history

Click Learn from history to scan your local history for distinctive
technical terms you have dictated more than once (camelCase, snake_case,
or terms with digits) and add them to your dictionary automatically.
Plain words are skipped, and terms you already have are not duplicated.

## Limits

You can store up to 100 dictionary entries and up to 100 snippets.
Over-long entries are trimmed rather than rejected, so a hand-edited
config never blocks Murmur from starting. A trigger can have at most 3
slots, and an expanded snippet is capped at 2000 characters after the
captured words are substituted in.
