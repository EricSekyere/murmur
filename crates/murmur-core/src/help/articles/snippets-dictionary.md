# Snippets and personal dictionary

Murmur has two ways to handle words and phrases it would otherwise get wrong:
text snippets that expand a spoken trigger, and a personal dictionary that biases
spelling. Both live in Settings.

## Text snippets

A snippet maps a spoken trigger to an expansion. Enter them in Settings, one per
line, as trigger = expansion. Say the trigger as a whole phrase and Murmur types
the expansion instead. For example, "my email" can expand to your address, or
"sign off" to a closing line.

## How snippet matching works

A snippet fires only when its trigger is the entire phrase, matched after
ignoring case, surrounding whitespace, and trailing punctuation. Saying the
trigger inside a longer sentence types the words normally. An empty or
punctuation-only trigger never fires, so silence cannot trigger an expansion.

## Snippet collisions

Built-in editing commands always win over a snippet with the same trigger, so a
snippet named "scratch that" would never fire and Murmur warns you. If two
snippets share a trigger, only the first one fires and the duplicate is flagged.
You can still type a snippet's words literally with the "literally" prefix.

## Personal dictionary

The personal dictionary is a list of names, jargon, and terms the model tends to
mishear. Enter them in Settings, one per line. On Whisper models they are
injected into the decoder as a glossary that biases recognition. On every
model, including Parakeet, a correction pass then fixes close mishearings of
your terms in the finished text, so the dictionary works no matter which engine
you use.

## How dictionary corrections work

After transcription, Murmur compares each word, and short runs of two or three
words, against your dictionary. A term is corrected only when it both sounds
like and is spelled close to the dictionary entry, so "kubernetis" becomes
Kubernetes and "git hub" becomes GitHub. A word that already matches an entry
except for casing gets just its casing fixed. Common English words are never
rewritten, and text with no near-match of a dictionary term is left untouched.

## Learn from history

Click Learn from history to scan your local history for distinctive technical
terms you have dictated more than once (camelCase, snake_case, or terms with
digits) and add them to your dictionary automatically. Plain words are skipped,
and terms you already have are not duplicated.

## Limits

You can store up to 100 dictionary entries and up to 100 snippets. Over-long
entries are trimmed rather than rejected, so a hand-edited config never blocks
Murmur from starting.
