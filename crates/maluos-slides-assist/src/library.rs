//! The user's presentations on disk (step 4.7): found by name and by what they say, so the
//! assistant can open "the deck about onboarding" from the start screen.
//!
//! Roots: `MALUOS_SLIDES_LIBRARY` (folders separated by `:`), else the XDG documents folder
//! and `~/Desktop`; the folders of the recent presentations are always included. Every
//! `.pptx`, `.ppsx`, `.potx` and `.deckcraft` under a root ([`MAX_DEPTH`] levels deep, hidden
//! folders skipped, at most [`MAX_FILES`] files) is read once and its text (the deck title, the
//! slide titles, body text, table cells, notes) kept in a cache file under
//! `$XDG_CACHE_HOME/maluos-slides/library.json`, keyed on the path with the file's modification
//! time and size, so a search after the first one parses nothing. Nothing is read outside the
//! roots and the recent files, and nothing is written anywhere but the cache
//! (`MALUOS_SLIDES_NO_PREFS` turns the cache off).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use maluos_slides_engine::model::{Presentation, Shape, ShapeKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// File types the library reads: what `file.open` reads as a presentation.
pub const EXTENSIONS: &[&str] = &["pptx", "ppsx", "potx", "deckcraft"];
/// How deep under a root the walk goes.
pub const MAX_DEPTH: usize = 5;
/// Files looked at per refresh, over all roots.
pub const MAX_FILES: usize = 2000;
/// Bytes of text kept per deck.
pub const TEXT_BYTES: usize = 32 * 1024;
/// The environment variable that names the library folders.
pub const ROOTS_ENV: &str = "MALUOS_SLIDES_LIBRARY";
const CACHE_VERSION: u32 = 1;

/// One presentation as the library knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Deck {
    pub path: String,
    /// The file name.
    pub name: String,
    /// The document's title property, else the first slide's title.
    pub title: String,
    pub slides: usize,
    /// Every slide's title, in order.
    pub titles: Vec<String>,
    /// The text of the deck, one line per slide part (`slide N: title`, body lines, `notes:`).
    pub text: String,
    /// Seconds since the epoch.
    pub modified: u64,
    pub size: u64,
    /// Set when the file could not be read; kept so it is not retried until it changes.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Cache {
    version: u32,
    decks: BTreeMap<String, Deck>,
}

/// What a refresh did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Refresh {
    /// Files found under the roots (and recent files elsewhere).
    pub files: usize,
    /// Files parsed this time.
    pub parsed: usize,
    /// Files not parsed because the time budget ran out.
    pub pending: usize,
    /// Files whose cache entry was reused.
    pub cached: usize,
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit<'a> {
    pub deck: &'a Deck,
    pub score: usize,
    /// The line of the deck's text with the first match, cut; empty when the match was the name.
    pub snippet: String,
    /// Position in the recent list when the deck is there (0 is the newest).
    pub recent: Option<usize>,
}

/// The library: its roots, the recent list, and the cache.
#[derive(Debug)]
pub struct Library {
    pub roots: Vec<PathBuf>,
    /// Recent presentations, newest first: listed first, and their folders are roots.
    pub recent: Vec<String>,
    cache_path: Option<PathBuf>,
    cache: Cache,
    loaded: bool,
}

/// Roots and cache installed with [`Library::set_override`]: tests and a host pointed elsewhere.
static OVERRIDE: Mutex<Option<(Vec<PathBuf>, Option<PathBuf>)>> = Mutex::new(None);

impl Library {
    /// Makes [`Library::discover`] use `roots` and `cache` instead of the environment (`None`
    /// restores it). Tests use it; so can a host that knows where the user's decks are.
    pub fn set_override(roots: Option<(Vec<PathBuf>, Option<PathBuf>)>) {
        if let Ok(mut guard) = OVERRIDE.lock() {
            *guard = roots;
        }
    }

    /// The library as configured: `MALUOS_SLIDES_LIBRARY`, else the documents and desktop
    /// folders; the recent files' folders; the cache in the user's cache folder.
    pub fn discover(recent: &[String]) -> Library {
        if let Ok(guard) = OVERRIDE.lock()
            && let Some((roots, cache)) = guard.as_ref()
        {
            let mut lib = Library::with_roots(roots.clone(), cache.clone());
            lib.recent = recent.to_vec();
            return lib;
        }
        let mut roots = Vec::new();
        match std::env::var(ROOTS_ENV).ok().filter(|v| !v.trim().is_empty()) {
            Some(v) => roots.extend(v.split(':').map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from)),
            None => {
                if let Some(d) = documents_dir() {
                    roots.push(d);
                }
                if let Some(h) = home_dir() {
                    roots.push(h.join("Desktop"));
                }
            }
        }
        for p in recent {
            if let Some(dir) = Path::new(p).parent().filter(|d| !d.as_os_str().is_empty()) {
                roots.push(dir.to_path_buf());
            }
        }
        let mut lib = Library::with_roots(roots, cache_path());
        lib.recent = recent.to_vec();
        lib
    }

    /// A library over `roots` with its cache at `cache_path` (none: nothing is saved). Roots
    /// that do not exist or repeat are dropped.
    pub fn with_roots(roots: Vec<PathBuf>, cache_path: Option<PathBuf>) -> Library {
        let mut seen = Vec::new();
        for r in roots {
            let Ok(canon) = r.canonicalize() else { continue };
            if canon.is_dir() && !seen.contains(&canon) {
                seen.push(canon);
            }
        }
        Library { roots: seen, recent: vec![], cache_path, cache: Cache::default(), loaded: false }
    }

    /// The decks the cache knows (after a refresh), newest first.
    pub fn decks(&self) -> Vec<&Deck> {
        let mut v: Vec<&Deck> = self.cache.decks.values().filter(|d| d.error.is_none()).collect();
        v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
        v
    }

    /// The presentation files: every one under the roots, plus the recent files wherever they
    /// are, with their modification time and size.
    pub fn files(&self) -> Vec<(PathBuf, u64, u64)> {
        let mut out: Vec<(PathBuf, u64, u64)> = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for p in &self.recent {
            let path = PathBuf::from(p);
            if let Some(f) = stat(&path)
                && seen.insert(path.clone())
            {
                out.push((path, f.0, f.1));
            }
        }
        for root in &self.roots {
            walk(root, 0, &mut |path| {
                if out.len() >= MAX_FILES {
                    return false;
                }
                if let Some(f) = stat(path)
                    && seen.insert(path.to_path_buf())
                {
                    out.push((path.to_path_buf(), f.0, f.1));
                }
                true
            });
        }
        out
    }

    /// Brings the cache up to date with the files: unchanged files keep their entry, changed
    /// and new ones are parsed until `budget` runs out, gone ones are dropped. The cache is
    /// saved when anything changed.
    pub fn refresh(&mut self, budget: Duration) -> Refresh {
        self.load();
        let start = Instant::now();
        let files = self.files();
        let mut r = Refresh { files: files.len(), ..Refresh::default() };
        let mut live = std::collections::BTreeSet::new();
        let mut changed = false;
        for (path, modified, size) in files {
            let key = path.to_string_lossy().to_string();
            live.insert(key.clone());
            if self.cache.decks.get(&key).is_some_and(|d| d.modified == modified && d.size == size) {
                r.cached += 1;
                continue;
            }
            if start.elapsed() > budget {
                r.pending += 1;
                continue;
            }
            self.cache.decks.insert(key.clone(), index_file(&key, modified, size));
            r.parsed += 1;
            changed = true;
        }
        let before = self.cache.decks.len();
        self.cache.decks.retain(|k, _| live.contains(k));
        changed |= self.cache.decks.len() != before;
        if changed {
            self.save();
        }
        r
    }

    /// Decks matching `query`: every word must occur in the name, the title, a slide title or
    /// the text (case-insensitive); the best matches first (name and title count most), newer
    /// first among equals. An empty query lists the recent decks first, then the rest newest
    /// first. At most `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit<'_>> {
        let tokens = tokens(query);
        let mut hits: Vec<Hit<'_>> = Vec::new();
        for deck in self.cache.decks.values() {
            if deck.error.is_some() {
                continue;
            }
            let recent = self.recent.iter().position(|p| *p == deck.path);
            if tokens.is_empty() {
                hits.push(Hit { deck, score: 0, snippet: String::new(), recent });
                continue;
            }
            let name = deck.name.to_lowercase();
            let title = deck.title.to_lowercase();
            let titles = deck.titles.join("\n").to_lowercase();
            let text = deck.text.to_lowercase();
            let mut score = 0;
            let mut first_text_hit: Option<usize> = None;
            for t in &tokens {
                let n = count(&name, t) * 5 + count(&title, t) * 5 + count(&titles, t) * 3 + count(&text, t);
                if n == 0 {
                    score = 0;
                    break;
                }
                score += n;
                if let Some(i) = text.find(t.as_str()) {
                    first_text_hit = Some(first_text_hit.map_or(i, |f| f.min(i)));
                }
            }
            if score == 0 {
                continue;
            }
            let snippet = first_text_hit.map(|i| snippet_at(&deck.text, i)).unwrap_or_default();
            hits.push(Hit { deck, score, snippet, recent });
        }
        if tokens.is_empty() {
            hits.sort_by(|a, b| match (a.recent, b.recent) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.deck.modified.cmp(&a.deck.modified).then_with(|| a.deck.path.cmp(&b.deck.path)),
            });
        } else {
            hits.sort_by(|a, b| {
                b.score.cmp(&a.score).then_with(|| b.deck.modified.cmp(&a.deck.modified)).then_with(|| a.deck.path.cmp(&b.deck.path))
            });
        }
        hits.truncate(limit);
        hits
    }

    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let Some(p) = &self.cache_path else { return };
        let Ok(text) = std::fs::read_to_string(p) else { return };
        if let Ok(c) = serde_json::from_str::<Cache>(&text)
            && c.version == CACHE_VERSION
        {
            self.cache = c;
        }
    }

    fn save(&mut self) {
        let Some(p) = &self.cache_path else { return };
        self.cache.version = CACHE_VERSION;
        let Ok(text) = serde_json::to_vec(&self.cache) else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, p)).is_err() {
            log::warn!("library cache {}: not written", p.display());
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// `hit` for a tool result or the pane.
pub fn hit_json(h: &Hit<'_>) -> Value {
    let d = h.deck;
    let folder = Path::new(&d.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let mut v = json!({
        "path": d.path, "name": d.name, "folder": folder, "title": d.title, "slides": d.slides,
        "modified": crate::memory::date_of(d.modified), "sizeBytes": d.size,
    });
    if !h.snippet.is_empty() {
        v["snippet"] = json!(h.snippet);
    }
    if let Some(i) = h.recent {
        v["recent"] = json!(i + 1);
    }
    v
}

/// The deck's title, its slide titles and its text, as the index keeps them.
pub fn deck_text(doc: &Presentation) -> (String, Vec<String>, String) {
    let mut titles = Vec::with_capacity(doc.slides.len());
    let mut text = String::new();
    for (i, sl) in doc.slides.iter().enumerate() {
        let title = sl.title();
        push_line(&mut text, &format!("slide {}: {}", i + 1, title));
        titles.push(title);
        for sh in &sl.shapes {
            shape_text(sh, &mut text);
        }
        let notes = sl.notes_text();
        if !notes.trim().is_empty() {
            push_line(&mut text, &format!("notes: {}", notes.split_whitespace().collect::<Vec<_>>().join(" ")));
        }
        if text.len() > TEXT_BYTES {
            break;
        }
    }
    if text.len() > TEXT_BYTES {
        let mut cut = TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    let title = if doc.props.title.trim().is_empty() { titles.first().cloned().unwrap_or_default() } else { doc.props.title.trim().to_string() };
    (title, titles, text)
}

fn shape_text(sh: &Shape, out: &mut String) {
    if let Some(t) = &sh.text {
        for line in t.text().lines() {
            push_line(out, line);
        }
    }
    match &sh.kind {
        ShapeKind::Table(t) => {
            for row in &t.rows {
                let cells: Vec<String> = row.cells.iter().map(|c| c.text.text().split_whitespace().collect::<Vec<_>>().join(" ")).collect();
                push_line(out, &cells.join(" | "));
            }
        }
        ShapeKind::Group { children, .. } => {
            for c in children {
                shape_text(c, out);
            }
        }
        _ => {}
    }
}

fn push_line(out: &mut String, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(line);
}

#[cfg(not(target_arch = "wasm32"))]
fn index_file(path: &str, modified: u64, size: u64) -> Deck {
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    match maluos_slides_engine::cmd::file::read_presentation_file(path) {
        Ok(f) => {
            let (title, titles, text) = deck_text(&f.doc);
            Deck { path: path.to_string(), name, title, slides: f.doc.slides.len(), titles, text, modified, size, error: None }
        }
        Err(e) => Deck { path: path.to_string(), name, modified, size, error: Some(e.to_string()), ..Deck::default() },
    }
}

#[cfg(target_arch = "wasm32")]
fn index_file(path: &str, modified: u64, size: u64) -> Deck {
    Deck { path: path.to_string(), modified, size, error: Some("no files on the web".into()), ..Deck::default() }
}

/// Modification time (seconds since the epoch) and size of a presentation file.
fn stat(path: &Path) -> Option<(u64, u64)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    Some((modified, meta.len()))
}

/// Calls `f` for every file under `dir` (sorted, hidden entries and symlinks skipped) until it
/// returns false.
fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path) -> bool) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return true };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = e.path();
        if ft.is_dir() {
            if !walk(&path, depth + 1, f) {
                return false;
            }
        } else if ft.is_file() && !f(&path) {
            return false;
        }
    }
    true
}

/// Search words: lowercase, punctuation trimmed, empty ones dropped.
pub fn tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()).filter(|w| !w.is_empty()).collect()
}

fn count(hay: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    hay.matches(needle).count()
}

/// The line of `text` around byte `at`, cut to 160 characters.
fn snippet_at(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[at..].find('\n').map(|i| at + i).unwrap_or(text.len());
    let line = text[start..end].trim();
    let n = line.chars().count();
    if n <= 160 { line.to_string() } else { line.chars().take(159).chain(std::iter::once('…')).collect() }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| !h.as_os_str().is_empty())
}

/// The XDG documents folder (`~/.config/user-dirs.dirs`), else `~/Documents`.
fn documents_dir() -> Option<PathBuf> {
    let home = home_dir()?;
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("XDG_DOCUMENTS_DIR=") {
                let v = rest.trim().trim_matches('"').replace("$HOME", &home.to_string_lossy());
                if !v.is_empty() {
                    return Some(PathBuf::from(v));
                }
            }
        }
    }
    Some(home.join("Documents"))
}

/// `$XDG_CACHE_HOME/maluos-slides/library.json`, none under `MALUOS_SLIDES_NO_PREFS`.
fn cache_path() -> Option<PathBuf> {
    if std::env::var_os("MALUOS_SLIDES_NO_PREFS").is_some() {
        return None;
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".cache")))
        .map(|d| d.join("maluos-slides").join("library.json"))
}
