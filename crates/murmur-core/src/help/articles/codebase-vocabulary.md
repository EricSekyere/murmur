# Codebase vocabulary

Codebase vocabulary scans your project folders and biases transcription toward
their identifiers, so symbols like calculateTotalRevenue or IndexerSettings
transcribe correctly. Turn it on in Settings under Codebase Vocabulary. It is off
by default.

## How it works

Murmur walks each project folder you add, extracts the distinctive identifiers
(function, class, and variable names), ranks them, and injects the top ones into
the decoder vocabulary. Decoder biasing helps Whisper models; on every model,
including Parakeet, the same sound-alike correction pass as the personal
dictionary fixes close mishearings of your identifiers in the finished text, so
"user controller" can come out as UserController.

## Add project folders

Click Add Folder in Settings and pick a project root. You can add several roots,
and their symbols share one ranked budget. Indexing only runs when at least one
folder is set and the feature is enabled. Remove a folder to drop its symbols.

## Automatic re-indexing

A file watcher re-indexes automatically when source files change, so the
vocabulary stays current as you work. You do not need to rescan by hand after
editing code.

## What gets scanned

The scan respects your .gitignore, skips hidden files, and only reads source
files by extension (Rust, TypeScript, JavaScript, Python, Go, and Java by
default). Very large files and binaries are skipped, and there is a cap on how
many files are scanned to keep it fast on large trees.

## Symbol budget

Only a ranked, capped subset of symbols is injected (up to 64 by default, and at
most 128). Keeping the list small fits the decoder prompt and avoids diluting the
bias. Identifiers from recently changed files are weighted more heavily.

## Accuracy of extraction

When built with tree-sitter support, identifier extraction is AST-accurate and
skips comments, strings, and keywords for Rust, Python, JavaScript, TypeScript,
Go, and Java. Other languages fall back to a lexical scan. Either way, the goal is
the same: bias toward the symbols that actually appear in your code.
