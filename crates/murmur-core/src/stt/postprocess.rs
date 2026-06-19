use std::collections::HashMap;
use std::sync::LazyLock;

/// Post-processes STT output for developer mode: removes fillers, expands
/// spoken symbols, corrects tech terms, and applies casing formatters.
pub struct PostProcessor;

impl PostProcessor {
    /// Run the full post-processing pipeline on raw STT text.
    pub fn process(text: &str) -> String {
        let text = remove_fillers(text);
        let text = expand_symbols(&text);
        let text = correct_tech_terms(&text);
        let text = apply_casing_formatters(&text);
        cleanup_whitespace(&text)
    }

    /// Prose cleanup for ordinary (non-developer) dictation: strip clear
    /// disfluencies and format spoken enumerations, but leave code transforms
    /// (symbol expansion, tech-term correction, casing commands) alone.
    pub fn process_prose(text: &str) -> String {
        let text = remove_disfluencies(text);
        // List formatting runs last: it introduces newlines that
        // cleanup_whitespace would otherwise collapse back into spaces.
        let text = cleanup_whitespace(&text);
        format_spoken_lists(&text)
    }
}

// ─── Spoken list formatting ──────────────────────────────────────────────────

/// Number words 0–10, index = value ("one" → 1).
const NUMBER_WORDS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
];

/// Turn a dictated "number one … number two …" enumeration into a numbered
/// list. Deliberately conservative: it only fires on the explicit "number N"
/// form (rare in flowing prose) with at least two markers whose values run
/// 1, 2, 3, … so ordinary sentences are never reshaped.
fn format_spoken_lists(text: &str) -> String {
    let markers = number_markers(text);
    if markers.len() < 2 || !markers.iter().enumerate().all(|(i, m)| m.2 == i as u32 + 1) {
        return text.to_string();
    }

    let mut out = String::new();
    let lead = text[..markers[0].0].trim();
    if !lead.is_empty() {
        out.push_str(lead);
        out.push('\n');
    }
    for (i, m) in markers.iter().enumerate() {
        let seg_end = markers.get(i + 1).map_or(text.len(), |next| next.0);
        let item = text[m.1..seg_end]
            .trim()
            .trim_start_matches([',', '.', ':'])
            .trim();
        out.push_str(&format!("{}. {}", m.2, item));
        if i + 1 < markers.len() {
            out.push('\n');
        }
    }
    out
}

/// Byte ranges and values of "number N" markers: (start of "number", end of the
/// count word, value).
fn number_markers(text: &str) -> Vec<(usize, usize, u32)> {
    let words = word_positions(text);
    let mut markers = Vec::new();
    let mut i = 0;
    while i + 1 < words.len() {
        if words[i].1.eq_ignore_ascii_case("number")
            && let Some(val) = parse_count(words[i + 1].1)
        {
            let end = words[i + 1].0 + words[i + 1].1.len();
            markers.push((words[i].0, end, val));
            i += 2;
            continue;
        }
        i += 1;
    }
    markers
}

/// (byte offset, word) for each whitespace-separated token.
fn word_positions(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < text.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push((start, &text[start..i]));
    }
    out
}

/// A count word ("one"…"ten") or a small digit, ignoring trailing punctuation.
fn parse_count(word: &str) -> Option<u32> {
    let w = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    if let Ok(n) = w.parse::<u32>() {
        return (1..=99).contains(&n).then_some(n);
    }
    NUMBER_WORDS
        .iter()
        .position(|n| *n == w)
        .map(|p| p as u32)
        .filter(|&v| v >= 1)
}

// ─── Filler Removal ──────────────────────────────────────────────────────────

/// Single-word fillers (matched case-insensitively, whole-word only).
const SINGLE_FILLERS: &[&str] = &["um", "uh", "uhh", "umm", "hmm", "er", "ah"];

/// Multi-word fillers removed unconditionally.
const MULTI_FILLERS: &[&str] = &["you know", "i mean", "basically", "actually", "literally"];

fn remove_fillers(text: &str) -> String {
    let mut result = text.to_string();

    // Remove multi-word fillers first (case-insensitive)
    for filler in MULTI_FILLERS {
        result = remove_phrase_ci(&result, filler);
    }

    result = remove_leading_so(&result);

    // "like" only when preceded by comma or at sentence start
    result = remove_like_filler(&result);

    for filler in SINGLE_FILLERS {
        result = remove_word_ci(&result, filler);
    }

    result
}

/// Drop a leading "so" disfluency ("so, let's…" → "let's…"), only at the very
/// start and only when followed by whitespace/comma so real words ("solo") are
/// untouched.
fn remove_leading_so(text: &str) -> String {
    let trimmed = text.trim_start();
    if !starts_with_word_ci(trimmed, "so") {
        return text.to_string();
    }
    let after = &trimmed[2..];
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with(',')) {
        return text.to_string();
    }
    let prefix_ws = text.len() - trimmed.len();
    let skip = if after.starts_with(", ") {
        4
    } else if after.starts_with(',') || after.starts_with(' ') {
        3
    } else {
        2
    };
    format!(
        "{}{}",
        &text[..prefix_ws],
        &trimmed[skip.min(trimmed.len())..]
    )
}

/// Multi-word disfluencies safe to drop from prose. Unlike the developer-mode
/// MULTI_FILLERS, this omits "actually" / "literally" / "basically", which
/// carry meaning in ordinary writing.
const PROSE_MULTI_FILLERS: &[&str] = &["you know", "i mean"];

/// Prose-safe filler removal: clear verbal disfluencies only, keeping words
/// that are meaningful outside dictating code.
fn remove_disfluencies(text: &str) -> String {
    // "you know" / "i mean" are only dropped when they stand alone as a
    // comma-bracketed aside, so a meaningful "do you know the answer" survives.
    let mut result = remove_parenthetical_fillers(text, PROSE_MULTI_FILLERS);
    result = remove_leading_so(&result);
    result = remove_like_filler(&result);
    for filler in SINGLE_FILLERS {
        result = remove_word_ci(&result, filler);
    }
    result
}

/// Remove a multi-word filler only when it is a stand-alone, comma-bracketed
/// aside ("I think, you know, it's fine" → "I think, it's fine"). A
/// grammatically integrated use ("do you know the answer") is never a lone
/// comma segment, so it is left untouched. Requires the model to have
/// punctuated the aside, which it reliably does for a spoken pause.
fn remove_parenthetical_fillers(text: &str, fillers: &[&str]) -> String {
    if !text.contains(',') {
        return text.to_string();
    }
    let segments: Vec<&str> = text.split(',').collect();
    let kept: Vec<&str> = segments
        .iter()
        .copied()
        .filter(|seg| {
            let norm = seg.trim().to_ascii_lowercase();
            !fillers.contains(&norm.as_str())
        })
        .collect();
    if kept.len() == segments.len() {
        return text.to_string();
    }
    kept.iter().map(|s| s.trim()).collect::<Vec<_>>().join(", ")
}

/// Push the UTF-8 character starting at byte `i` (a char boundary) onto
/// `result` and return the index just past it. Avoids `bytes[i] as char`,
/// which corrupts multibyte characters by treating a continuation byte as a
/// Latin-1 char.
fn push_char_at(result: &mut String, text: &str, i: usize) -> usize {
    match text[i..].chars().next() {
        Some(ch) => {
            result.push(ch);
            i + ch.len_utf8()
        }
        None => i + 1,
    }
}

/// Remove a phrase (case-insensitive, whole-word boundaries) from text.
fn remove_phrase_ci(text: &str, phrase: &str) -> String {
    // ASCII-lowercase is length-preserving, so `lower` stays byte-aligned with
    // `text` (a Unicode to_lowercase can change byte length, e.g. İ → i̇, which
    // would desync the offset-indexed match below). Fillers are ASCII words.
    let lower = text.to_ascii_lowercase();
    let phrase_lower = phrase.to_ascii_lowercase();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    let bytes = text.as_bytes();
    let plen = phrase_lower.len();

    while i < text.len() {
        if lower.get(i..i + plen).is_some_and(|s| s == phrase_lower)
            && is_word_boundary(bytes, i)
            && is_word_boundary_end(bytes, i + plen)
        {
            // Skip optional trailing comma+space or just space
            let after = i + plen;
            if text[after..].starts_with(", ") {
                i = after + 2;
            } else if text[after..].starts_with(' ') {
                i = after + 1;
            } else {
                i = after;
            }
        } else {
            i = push_char_at(&mut result, text, i);
        }
    }

    result
}

/// Remove a single word (case-insensitive, whole-word boundaries).
fn remove_word_ci(text: &str, word: &str) -> String {
    remove_phrase_ci(text, word)
}

/// Remove "like" only when preceded by a comma or at the start of a sentence.
fn remove_like_filler(text: &str) -> String {
    // ASCII-lowercase keeps `lower` byte-aligned with `text` (see remove_phrase_ci).
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < text.len() {
        if lower.get(i..i + 4) == Some("like")
            && is_word_boundary(bytes, i)
            && is_word_boundary_end(bytes, i + 4)
        {
            // Check if preceded by comma or at start
            let before = &text[..i].trim_end();
            let at_start = before.is_empty()
                || before.ends_with('.')
                || before.ends_with('!')
                || before.ends_with('?');
            let after_comma = before.ends_with(',');

            if at_start || after_comma {
                // Remove the comma before "like" if present
                if after_comma {
                    // Trim trailing ", " or ","
                    let trimmed = result.trim_end().to_string();
                    result = if trimmed.ends_with(',') {
                        format!("{} ", &trimmed[..trimmed.len() - 1])
                    } else {
                        format!("{} ", trimmed)
                    };
                }
                // Skip "like" + trailing space
                i += 4;
                if i < text.len() && bytes[i] == b' ' {
                    i += 1;
                }
                continue;
            }
        }
        i = push_char_at(&mut result, text, i);
    }

    result
}

fn starts_with_word_ci(text: &str, word: &str) -> bool {
    let lower = text.to_lowercase();
    lower.starts_with(&word.to_lowercase())
        && (text.len() == word.len() || !text.as_bytes()[word.len()].is_ascii_alphanumeric())
}

fn is_word_boundary(bytes: &[u8], pos: usize) -> bool {
    pos == 0 || !bytes[pos - 1].is_ascii_alphanumeric()
}

fn is_word_boundary_end(bytes: &[u8], pos: usize) -> bool {
    pos >= bytes.len() || !bytes[pos].is_ascii_alphanumeric()
}

// ─── Symbol Expansion ────────────────────────────────────────────────────────

/// Ordered longest-first for greedy matching.
static SYMBOL_MAP: LazyLock<Vec<(&str, &str)>> = LazyLock::new(|| {
    vec![
        // Multi-word operators (longest first)
        ("triple equals", "==="),
        ("double equals", "=="),
        ("not equals", "!="),
        ("strict not equals", "!=="),
        ("greater than or equal", ">="),
        ("less than or equal", "<="),
        ("double ampersand", "&&"),
        ("double pipe", "||"),
        ("double colon", "::"),
        ("double slash", "//"),
        ("fat arrow", "=>"),
        ("thin arrow", "->"),
        ("spread operator", "..."),
        ("null coalescing", "??"),
        ("optional chaining", "?."),
        ("left shift", "<<"),
        ("right shift", ">>"),
        ("plus equals", "+="),
        ("minus equals", "-="),
        ("times equals", "*="),
        ("divide equals", "/="),
        ("plus plus", "++"),
        ("minus minus", "--"),
        // Brackets/parens
        ("open paren", "("),
        ("close paren", ")"),
        ("open parenthesis", "("),
        ("close parenthesis", ")"),
        ("open bracket", "["),
        ("close bracket", "]"),
        ("open square bracket", "["),
        ("close square bracket", "]"),
        ("open brace", "{"),
        ("close brace", "}"),
        ("open curly", "{"),
        ("close curly", "}"),
        ("open angle", "<"),
        ("close angle", ">"),
        // Single-word symbols
        ("semicolon", ";"),
        ("colon", ":"),
        ("comma", ","),
        ("period", "."),
        ("dot", "."),
        ("exclamation", "!"),
        ("bang", "!"),
        ("question mark", "?"),
        ("at sign", "@"),
        ("hash", "#"),
        ("dollar sign", "$"),
        ("percent", "%"),
        ("caret", "^"),
        ("ampersand", "&"),
        ("asterisk", "*"),
        ("star", "*"),
        ("pipe", "|"),
        ("tilde", "~"),
        ("backtick", "`"),
        ("backslash", "\\"),
        ("forward slash", "/"),
        ("slash", "/"),
        ("underscore", "_"),
        ("hyphen", "-"),
        ("dash", "-"),
        ("equals", "="),
        ("plus", "+"),
        ("minus", "-"),
        ("greater than", ">"),
        ("less than", "<"),
        ("single quote", "'"),
        ("double quote", "\""),
        ("new line", "\n"),
        ("newline", "\n"),
        ("tab", "\t"),
        ("space", " "),
    ]
});

fn expand_symbols(text: &str) -> String {
    // ASCII-lowercase keeps `lower` byte-aligned with `text` (see remove_phrase_ci);
    // spoken-symbol keys are ASCII, so a length-changing char no longer desyncs
    // the offset-indexed match and silently drops the expansion.
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut i = 0;

    while i < text.len() {
        let mut matched = false;

        for &(spoken, symbol) in SYMBOL_MAP.iter() {
            let slen = spoken.len();
            if lower.get(i..i + slen) == Some(spoken)
                && is_word_boundary(bytes, i)
                && is_word_boundary_end(bytes, i + slen)
            {
                result.push_str(symbol);
                i += slen;
                // Skip trailing space if the symbol itself isn't whitespace
                if !symbol.trim().is_empty() && i < text.len() && bytes[i] == b' ' {
                    i += 1;
                }
                matched = true;
                break;
            }
        }

        if !matched {
            // Advance by whole UTF-8 chars; `bytes[i] as char` corrupts non-ASCII.
            i = push_char_at(&mut result, text, i);
        }
    }

    result
}

// ─── Tech Term Correction ────────────────────────────────────────────────────

static TECH_TERMS: LazyLock<HashMap<String, &str>> = LazyLock::new(|| {
    let entries: Vec<(&str, &str)> = vec![
        // Languages
        ("javascript", "JavaScript"),
        ("java script", "JavaScript"),
        ("typescript", "TypeScript"),
        ("type script", "TypeScript"),
        ("python", "Python"),
        ("golang", "Go"),
        ("go lang", "Go"),
        ("csharp", "C#"),
        ("c sharp", "C#"),
        ("c plus plus", "C++"),
        ("cplusplus", "C++"),
        ("rust", "Rust"),
        ("kotlin", "Kotlin"),
        ("swift", "Swift"),
        ("ruby", "Ruby"),
        ("php", "PHP"),
        ("scala", "Scala"),
        ("elixir", "Elixir"),
        ("haskell", "Haskell"),
        ("clojure", "Clojure"),
        ("lua", "Lua"),
        ("perl", "Perl"),
        ("zig", "Zig"),
        ("dart", "Dart"),
        // Frameworks & Libraries
        ("react", "React"),
        ("next js", "Next.js"),
        ("nextjs", "Next.js"),
        ("node js", "Node.js"),
        ("nodejs", "Node.js"),
        ("vue js", "Vue.js"),
        ("vuejs", "Vue.js"),
        ("nuxt js", "Nuxt.js"),
        ("nuxtjs", "Nuxt.js"),
        ("nest js", "NestJS"),
        ("nestjs", "NestJS"),
        ("express js", "Express.js"),
        ("expressjs", "Express.js"),
        ("angular", "Angular"),
        ("svelte", "Svelte"),
        ("django", "Django"),
        ("flask", "Flask"),
        ("fastapi", "FastAPI"),
        ("fast api", "FastAPI"),
        ("spring boot", "Spring Boot"),
        ("tailwind", "Tailwind"),
        ("tailwind css", "Tailwind CSS"),
        ("bootstrap", "Bootstrap"),
        ("jquery", "jQuery"),
        ("three js", "Three.js"),
        ("threejs", "Three.js"),
        ("electron", "Electron"),
        ("tauri", "Tauri"),
        ("remix", "Remix"),
        ("gatsby", "Gatsby"),
        ("astro", "Astro"),
        // React hooks
        ("use state", "useState"),
        ("use effect", "useEffect"),
        ("use memo", "useMemo"),
        ("use callback", "useCallback"),
        ("use ref", "useRef"),
        ("use context", "useContext"),
        ("use reducer", "useReducer"),
        ("use layout effect", "useLayoutEffect"),
        // Acronyms & protocols
        ("api", "API"),
        ("apis", "APIs"),
        ("sql", "SQL"),
        ("html", "HTML"),
        ("css", "CSS"),
        ("json", "JSON"),
        ("yaml", "YAML"),
        ("toml", "TOML"),
        ("xml", "XML"),
        ("csv", "CSV"),
        ("http", "HTTP"),
        ("https", "HTTPS"),
        ("jwt", "JWT"),
        ("oauth", "OAuth"),
        ("cli", "CLI"),
        ("gui", "GUI"),
        ("sdk", "SDK"),
        ("cdn", "CDN"),
        ("dns", "DNS"),
        ("tcp", "TCP"),
        ("udp", "UDP"),
        ("ssh", "SSH"),
        ("ssl", "SSL"),
        ("tls", "TLS"),
        ("ftp", "FTP"),
        ("wasm", "WASM"),
        ("webassembly", "WebAssembly"),
        ("web assembly", "WebAssembly"),
        ("graphql", "GraphQL"),
        ("graph ql", "GraphQL"),
        ("grpc", "gRPC"),
        ("rest", "REST"),
        ("restful", "RESTful"),
        ("ci cd", "CI/CD"),
        ("cicd", "CI/CD"),
        ("url", "URL"),
        ("uri", "URI"),
        ("uuid", "UUID"),
        ("regex", "regex"),
        ("ascii", "ASCII"),
        ("utf", "UTF"),
        ("utf8", "UTF-8"),
        ("utf 8", "UTF-8"),
        ("ide", "IDE"),
        ("orm", "ORM"),
        ("mvc", "MVC"),
        ("oop", "OOP"),
        ("crud", "CRUD"),
        ("cors", "CORS"),
        ("csrf", "CSRF"),
        ("xss", "XSS"),
        // Tools & platforms
        ("github", "GitHub"),
        ("git hub", "GitHub"),
        ("gitlab", "GitLab"),
        ("git lab", "GitLab"),
        ("bitbucket", "Bitbucket"),
        ("vs code", "VS Code"),
        ("vscode", "VS Code"),
        ("neovim", "Neovim"),
        ("vim", "Vim"),
        ("docker", "Docker"),
        ("kubernetes", "Kubernetes"),
        ("kubectl", "kubectl"),
        ("webpack", "Webpack"),
        ("vite", "Vite"),
        ("eslint", "ESLint"),
        ("prettier", "Prettier"),
        ("babel", "Babel"),
        ("rollup", "Rollup"),
        ("turbopack", "Turbopack"),
        ("npm", "npm"),
        ("yarn", "yarn"),
        ("pnpm", "pnpm"),
        ("bun", "Bun"),
        ("deno", "Deno"),
        ("cargo", "Cargo"),
        ("pip", "pip"),
        ("homebrew", "Homebrew"),
        ("git", "Git"),
        ("jira", "Jira"),
        ("confluence", "Confluence"),
        ("notion", "Notion"),
        ("figma", "Figma"),
        ("storybook", "Storybook"),
        ("playwright", "Playwright"),
        ("cypress", "Cypress"),
        ("jest", "Jest"),
        ("vitest", "Vitest"),
        ("mocha", "Mocha"),
        // Databases
        ("postgres", "PostgreSQL"),
        ("postgresql", "PostgreSQL"),
        ("mysql", "MySQL"),
        ("my sql", "MySQL"),
        ("sqlite", "SQLite"),
        ("mongo db", "MongoDB"),
        ("mongodb", "MongoDB"),
        ("redis", "Redis"),
        ("supabase", "Supabase"),
        ("firebase", "Firebase"),
        ("dynamo db", "DynamoDB"),
        ("dynamodb", "DynamoDB"),
        ("prisma", "Prisma"),
        ("drizzle", "Drizzle"),
        // Cloud & DevOps
        ("aws", "AWS"),
        ("gcp", "GCP"),
        ("azure", "Azure"),
        ("vercel", "Vercel"),
        ("netlify", "Netlify"),
        ("heroku", "Heroku"),
        ("terraform", "Terraform"),
        ("ansible", "Ansible"),
        ("nginx", "Nginx"),
        ("apache", "Apache"),
        ("lambda", "Lambda"),
        ("cloudflare", "Cloudflare"),
        ("digitalocean", "DigitalOcean"),
        ("digital ocean", "DigitalOcean"),
        // AI tools
        ("cursor", "Cursor"),
        ("claude", "Claude"),
        ("copilot", "Copilot"),
        ("chatgpt", "ChatGPT"),
        ("chat gpt", "ChatGPT"),
        ("openai", "OpenAI"),
        ("open ai", "OpenAI"),
        ("anthropic", "Anthropic"),
        ("llm", "LLM"),
        ("llms", "LLMs"),
        // Common dev terms
        ("dev ops", "DevOps"),
        ("devops", "DevOps"),
        ("localhost", "localhost"),
        ("env", "env"),
        ("dotenv", "dotenv"),
        ("stdout", "stdout"),
        ("stderr", "stderr"),
        ("stdin", "stdin"),
        ("async", "async"),
        ("await", "await"),
        ("boolean", "boolean"),
        ("null", "null"),
        ("undefined", "undefined"),
        ("nan", "NaN"),
        ("int", "int"),
        ("frontend", "frontend"),
        ("backend", "backend"),
        ("fullstack", "fullstack"),
        ("full stack", "fullstack"),
        ("middleware", "middleware"),
        ("webhook", "webhook"),
        ("websocket", "WebSocket"),
        ("web socket", "WebSocket"),
    ];

    let mut map = HashMap::with_capacity(entries.len());
    for (key, val) in entries {
        map.insert(key.to_lowercase(), val);
    }
    map
});

fn correct_tech_terms(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = Vec::with_capacity(words.len());
    let mut i = 0;

    while i < words.len() {
        let mut matched = false;

        // Try 3-word window
        if i + 2 < words.len() {
            let key = format!(
                "{} {} {}",
                words[i].to_lowercase(),
                words[i + 1].to_lowercase(),
                words[i + 2].to_lowercase()
            );
            // Strip trailing punctuation for lookup
            let (lookup, suffix) = strip_trailing_punct(&key);
            if let Some(&replacement) = TECH_TERMS.get(&lookup) {
                result.push(format!("{}{}", replacement, suffix));
                i += 3;
                matched = true;
            }
        }

        // Try 2-word window
        if !matched && i + 1 < words.len() {
            let key = format!(
                "{} {}",
                words[i].to_lowercase(),
                words[i + 1].to_lowercase()
            );
            let (lookup, suffix) = strip_trailing_punct(&key);
            if let Some(&replacement) = TECH_TERMS.get(&lookup) {
                result.push(format!("{}{}", replacement, suffix));
                i += 2;
                matched = true;
            }
        }

        // Try 1-word
        if !matched {
            let lower = words[i].to_lowercase();
            let (lookup, suffix) = strip_trailing_punct(&lower);
            if let Some(&replacement) = TECH_TERMS.get(&lookup) {
                result.push(format!("{}{}", replacement, suffix));
            } else {
                result.push(words[i].to_string());
            }
            i += 1;
        }
    }

    result.join(" ")
}

/// Strip trailing punctuation for lookup, returning (clean, suffix).
fn strip_trailing_punct(s: &str) -> (String, &str) {
    let trimmed = s.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    let suffix_start = trimmed.len();
    (trimmed.to_string(), &s[suffix_start..])
}

// ─── Casing Formatters ───────────────────────────────────────────────────────

const CASING_KEYWORDS: &[&str] = &["camel", "snake", "pascal", "kebab", "upper"];

const STOP_WORDS: &[&str] = &[
    "and", "then", "or", "but", "the", "a", "an", "to", "in", "on", "for", "with", "from", "that",
    "this", "is", "it", "of",
];

fn apply_casing_formatters(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;

    while i < words.len() {
        let lower = words[i].to_lowercase();

        if let Some(format_kind) = CASING_KEYWORDS.iter().find(|&&kw| kw == lower) {
            // Collect words until next casing keyword, stop word, or end
            let mut collected: Vec<String> = Vec::new();
            i += 1;
            while i < words.len() {
                let w_lower = words[i].to_lowercase();
                if CASING_KEYWORDS.contains(&w_lower.as_str()) {
                    break;
                }
                if STOP_WORDS.contains(&w_lower.as_str()) {
                    break;
                }
                collected.push(w_lower);
                i += 1;
            }

            if collected.is_empty() {
                // No words to format — emit the keyword as-is
                result.push(words[i - 1].to_string());
                // i was already incremented past the keyword above,
                // but since collected is empty, we didn't increment further.
                // Actually we need to re-check: i was incremented at "i += 1" after
                // finding the keyword. Then the while loop didn't run because
                // the next word is a stop word or casing keyword or we're at end.
                // So i is already correct.
            } else {
                result.push(apply_case(format_kind, &collected));
            }
        } else {
            result.push(words[i].to_string());
            i += 1;
        }
    }

    result.join(" ")
}

fn apply_case(kind: &str, words: &[String]) -> String {
    match kind {
        "camel" => {
            let mut out = String::new();
            for (idx, w) in words.iter().enumerate() {
                if idx == 0 {
                    out.push_str(&w.to_lowercase());
                } else {
                    out.push_str(&capitalize(w));
                }
            }
            out
        }
        "pascal" => {
            let mut out = String::new();
            for w in words {
                out.push_str(&capitalize(w));
            }
            out
        }
        "snake" => words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("_"),
        "kebab" => words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("-"),
        "upper" => words
            .iter()
            .map(|w| w.to_uppercase())
            .collect::<Vec<_>>()
            .join("_"),
        _ => words.join(" "),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let upper: String = c.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

// ─── Whitespace Cleanup ──────────────────────────────────────────────────────

fn cleanup_whitespace(text: &str) -> String {
    // Collapse multiple spaces, trim
    let mut result = String::with_capacity(text.len());
    let mut prev_space = false;

    for ch in text.chars() {
        if ch == ' ' {
            if !prev_space && !result.is_empty() {
                result.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_symbols_stays_aligned_after_length_changing_lowercase() {
        // İ (U+0130) lowercases to a longer byte string; the offset-indexed
        // matcher must stay aligned and still expand "equals" rather than
        // silently skip it (the to_ascii_lowercase fix).
        let out = expand_symbols("İ equals x");
        assert!(out.contains('='), "expected expansion, got {out:?}");
        assert!(
            !out.contains("equals"),
            "'equals' should have been expanded: {out:?}"
        );
    }

    #[test]
    fn prose_strips_disfluencies_but_keeps_meaningful_words() {
        let out = PostProcessor::process_prose("um I think, you know, it works");
        let lower = out.to_lowercase();
        assert!(!lower.contains("um"), "{out:?}");
        assert!(!lower.contains("you know"), "{out:?}");
        assert!(
            out.contains("I think") && out.contains("it works"),
            "{out:?}"
        );
        // "actually"/"literally" are meaningful in prose (kept, unlike dev mode).
        let kept = PostProcessor::process_prose("I actually literally finished");
        assert!(
            kept.contains("actually") && kept.contains("literally"),
            "{kept:?}"
        );
        // "you know" used meaningfully (not a comma-bracketed aside) is kept.
        assert_eq!(
            PostProcessor::process_prose("do you know the answer"),
            "do you know the answer"
        );
        // "i mean" as a real clause is kept; as a parenthetical it's dropped.
        assert_eq!(
            PostProcessor::process_prose("I mean it when I say that"),
            "I mean it when I say that"
        );
    }

    #[test]
    fn spoken_number_list_only_fires_on_explicit_sequential_markers() {
        assert_eq!(
            PostProcessor::process_prose("my plan number one buy milk number two walk the dog"),
            "my plan\n1. buy milk\n2. walk the dog"
        );
        // Mentioning a number in prose is left alone.
        let prose = "the number one priority is shipping";
        assert_eq!(format_spoken_lists(prose), prose);
        // A single marker is not a list.
        assert_eq!(format_spoken_lists("number one thing"), "number one thing");
    }

    // ── Filler removal ───────────────────────────────────────────────────

    #[test]
    fn filler_removal_basic() {
        assert_eq!(
            remove_fillers("um I want to uh use this"),
            "I want to use this"
        );
    }

    #[test]
    fn filler_removal_multi_word() {
        assert_eq!(
            remove_fillers("you know I basically want this"),
            "I want this"
        );
    }

    #[test]
    fn filler_preserves_like_in_context() {
        assert_eq!(remove_fillers("I like React"), "I like React");
    }

    #[test]
    fn filler_removes_like_after_comma() {
        assert_eq!(
            remove_fillers("well, like I was saying"),
            "well I was saying"
        );
    }

    #[test]
    fn filler_removes_so_at_start() {
        assert_eq!(remove_fillers("so I want to code"), "I want to code");
    }

    #[test]
    fn filler_removal_preserves_non_ascii() {
        // Byte-as-char rewriting used to corrupt accents/non-Latin text.
        // Without a filler the text must pass through byte-for-byte.
        assert_eq!(
            remove_fillers("café déjà vu Iñtërnatização"),
            "café déjà vu Iñtërnatização"
        );
        // A filler is still removed and the surrounding accents survive.
        assert_eq!(remove_fillers("um café déjà vu"), "café déjà vu");
    }

    #[test]
    fn filler_preserves_so_mid_sentence() {
        assert_eq!(remove_fillers("I think so too"), "I think so too");
    }

    // ── Tech term correction ─────────────────────────────────────────────

    #[test]
    fn tech_terms_languages() {
        assert_eq!(
            correct_tech_terms("I use typescript and python"),
            "I use TypeScript and Python"
        );
    }

    #[test]
    fn tech_terms_acronyms() {
        assert_eq!(
            correct_tech_terms("the api returns json over http"),
            "the API returns JSON over HTTP"
        );
    }

    #[test]
    fn tech_terms_multi_word() {
        assert_eq!(
            correct_tech_terms("use next js with vs code"),
            "use Next.js with VS Code"
        );
    }

    #[test]
    fn tech_terms_react_hooks() {
        assert_eq!(
            correct_tech_terms("call use state and use effect"),
            "call useState and useEffect"
        );
    }

    #[test]
    fn tech_terms_with_punctuation() {
        assert_eq!(
            correct_tech_terms("I like typescript."),
            "I like TypeScript."
        );
    }

    // ── Symbol expansion ─────────────────────────────────────────────────

    #[test]
    fn symbol_parens() {
        assert_eq!(expand_symbols("open paren x close paren"), "(x )");
    }

    #[test]
    fn symbol_triple_equals() {
        assert_eq!(expand_symbols("x triple equals y"), "x ===y");
    }

    #[test]
    fn symbol_word_boundary() {
        // "dot" inside "dotnet" should NOT be expanded
        assert_eq!(expand_symbols("dotnet framework"), "dotnet framework");
    }

    #[test]
    fn symbol_fat_arrow() {
        assert_eq!(expand_symbols("fat arrow function"), "=>function");
    }

    #[test]
    fn symbol_expansion_preserves_non_ascii() {
        // Byte-as-char rewriting used to corrupt non-ASCII; it must pass through.
        assert_eq!(
            expand_symbols("café déjà vu Iñtërnatização 日本語 🚀"),
            "café déjà vu Iñtërnatização 日本語 🚀"
        );
        // A symbol still expands and the surrounding accents survive.
        assert_eq!(expand_symbols("café equals déjà"), "café =déjà");
    }

    // ── Casing formatters ────────────────────────────────────────────────

    #[test]
    fn casing_camel() {
        assert_eq!(
            apply_casing_formatters("camel get user name"),
            "getUserName"
        );
    }

    #[test]
    fn casing_snake() {
        assert_eq!(
            apply_casing_formatters("snake get user name"),
            "get_user_name"
        );
    }

    #[test]
    fn casing_pascal() {
        assert_eq!(
            apply_casing_formatters("pascal user service"),
            "UserService"
        );
    }

    #[test]
    fn casing_kebab() {
        assert_eq!(
            apply_casing_formatters("kebab my component"),
            "my-component"
        );
    }

    #[test]
    fn casing_upper() {
        assert_eq!(apply_casing_formatters("upper max retries"), "MAX_RETRIES");
    }

    #[test]
    fn casing_stops_at_stop_word() {
        assert_eq!(
            apply_casing_formatters("camel get user and then do stuff"),
            "getUser and then do stuff"
        );
    }

    #[test]
    fn casing_consecutive() {
        assert_eq!(
            apply_casing_formatters("camel get user snake set value"),
            "getUser set_value"
        );
    }

    // ── Full pipeline ────────────────────────────────────────────────────

    #[test]
    fn full_pipeline_basic() {
        let input = "um basically I want to use typescript with react";
        let result = PostProcessor::process(input);
        assert_eq!(result, "I want to use TypeScript with React");
    }

    #[test]
    fn full_pipeline_casing() {
        let input = "create a function called camel get user name";
        let result = PostProcessor::process(input);
        assert_eq!(result, "create a function called getUserName");
    }

    #[test]
    fn full_pipeline_symbols() {
        let input = "open paren x close paren triple equals y";
        let result = PostProcessor::process(input);
        assert_eq!(result, "(x )===y");
    }

    #[test]
    fn full_pipeline_empty() {
        assert_eq!(PostProcessor::process(""), "");
    }

    #[test]
    fn full_pipeline_no_changes() {
        let input = "hello world";
        assert_eq!(PostProcessor::process(input), "hello world");
    }

    // ── Performance ──────────────────────────────────────────────────────

    #[test]
    fn performance_100_iterations() {
        let input = "um basically I want to use typescript with react and call use state in the component to handle the api response from the backend";
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = PostProcessor::process(input);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "100 iterations took {}ms (limit: 500ms)",
            elapsed.as_millis()
        );
    }
}
