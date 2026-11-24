//! Console input history: persistence in
//! `$XDG_DATA_HOME/viscas/console_history` and Up/Down recall.
//!
//! The file holds one JSON string per line (entries may span several
//! lines: Shift+Enter scripts), oldest first, capped at
//! [`viscas_engine::HISTORY_LIMIT`]. Unreadable lines are skipped. Pure
//! logic plus plain file I/O, no GTK.

use std::io;
use std::path::{Path, PathBuf};

use viscas_engine::HISTORY_LIMIT;

/// `$XDG_DATA_HOME/viscas`, or `$HOME/.local/share/viscas`.
pub fn data_dir_from(xdg: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match (xdg.filter(|x| Path::new(x).is_absolute()), home) {
        (Some(x), _) => Some(PathBuf::from(x).join("viscas")),
        (None, Some(h)) if !h.is_empty() => Some(PathBuf::from(h).join(".local/share/viscas")),
        _ => None,
    }
}

/// The history file of this user, if a data directory is known.
pub fn history_path() -> Option<PathBuf> {
    let xdg = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    data_dir_from(xdg.as_deref(), home.as_deref()).map(|d| d.join("console_history"))
}

/// File contents for `history` (its newest [`HISTORY_LIMIT`] entries).
pub fn encode(history: &[String]) -> String {
    let skip = history.len().saturating_sub(HISTORY_LIMIT);
    let mut out = String::new();
    for entry in history.iter().skip(skip).filter(|e| !e.trim().is_empty()) {
        // Serializing a string cannot fail.
        out.push_str(&serde_json::to_string(entry).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// History entries in `text`, oldest first, capped.
pub fn decode(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<String>(l.trim()).ok())
        .filter(|e| !e.trim().is_empty())
        .collect();
    let n = entries.len();
    if n > HISTORY_LIMIT {
        entries.drain(..n - HISTORY_LIMIT);
    }
    entries
}

/// Reads the history at `path` (empty when missing or unreadable).
pub fn load(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|t| decode(&t))
        .unwrap_or_default()
}

/// `disk` followed by the `new` entries (blank ones and consecutive
/// repeats skipped, like the engine), capped to the newest
/// [`HISTORY_LIMIT`].
pub fn merged(mut disk: Vec<String>, new: &[String]) -> Vec<String> {
    for e in new {
        if !e.trim().is_empty() && disk.last() != Some(e) {
            disk.push(e.clone());
        }
    }
    let n = disk.len();
    if n > HISTORY_LIMIT {
        disk.drain(..n - HISTORY_LIMIT);
    }
    disk
}

/// Adds `new` entries to the history file. The file is re-read and
/// merged first, so several running instances append to each other's
/// history instead of overwriting it; the result is written atomically
/// (unique temp file, fsync, rename).
pub fn append(path: &Path, new: &[String]) -> io::Result<()> {
    let all = merged(load(path), new);
    write_atomic(path, &encode(&all))
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path
        .file_name()
        .map_or_else(|| "history".into(), |n| n.to_string_lossy().into_owned());
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Up/Down recall over the history. Starting to browse remembers the
/// current input (the draft) and only offers entries starting with it,
/// like fish; going past the newest entry restores the draft.
#[derive(Debug, Default)]
pub struct HistoryNav {
    /// Index of the entry shown, while browsing.
    index: Option<usize>,
    draft: String,
}

impl HistoryNav {
    /// Older matching entry, if any; `current` is the input text.
    pub fn up(&mut self, history: &[String], current: &str) -> Option<String> {
        if self.index.is_none() {
            self.draft = current.to_owned();
        }
        let before = self.index.unwrap_or(history.len()).min(history.len());
        let found = (0..before)
            .rev()
            .find(|&i| history[i].starts_with(&self.draft) && history[i] != current)?;
        self.index = Some(found);
        Some(history[found].clone())
    }

    /// Newer matching entry, or the draft after the newest one; `None`
    /// when not browsing.
    pub fn down(&mut self, history: &[String]) -> Option<String> {
        let from = self.index? + 1;
        match (from..history.len()).find(|&i| history[i].starts_with(&self.draft)) {
            Some(i) => {
                self.index = Some(i);
                Some(history[i].clone())
            }
            None => {
                self.index = None;
                Some(std::mem::take(&mut self.draft))
            }
        }
    }

    /// Stops browsing (the input was edited or evaluated).
    pub fn reset(&mut self) {
        self.index = None;
        self.draft.clear();
    }

    #[cfg(test)]
    pub fn is_browsing(&self) -> bool {
        self.index.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn data_dir_rules() {
        assert_eq!(
            data_dir_from(Some("/x"), Some("/home/u")),
            Some(PathBuf::from("/x/viscas"))
        );
        // Relative XDG paths are ignored, as the spec says.
        assert_eq!(
            data_dir_from(Some("rel"), Some("/home/u")),
            Some(PathBuf::from("/home/u/.local/share/viscas"))
        );
        assert_eq!(data_dir_from(None, None), None);
        assert_eq!(data_dir_from(None, Some("")), None);
    }

    #[test]
    fn round_trip_keeps_multiline_entries() {
        let entries = h(&["a := 3", "f(x) := x^2\nf(a)", "\"quoted\" \\ back"]);
        assert_eq!(decode(&encode(&entries)), entries);
    }

    #[test]
    fn decode_skips_garbage_and_blank() {
        let text = "\"ok\"\nnot json\n\"\"\n\n\"also ok\"\n";
        assert_eq!(decode(text), h(&["ok", "also ok"]));
    }

    #[test]
    fn capped_to_limit_keeping_newest() {
        let many: Vec<String> = (0..HISTORY_LIMIT + 25).map(|i| i.to_string()).collect();
        let back = decode(&encode(&many));
        assert_eq!(back.len(), HISTORY_LIMIT);
        assert_eq!(back[0], "25");
        assert_eq!(
            back.last().map(String::as_str),
            Some(&*many[many.len() - 1])
        );
        // A hand-edited oversized file is capped on load too.
        let text: String = many
            .iter()
            .map(|e| format!("{}\n", serde_json::to_string(e).unwrap()))
            .collect();
        assert_eq!(decode(&text).len(), HISTORY_LIMIT);
    }

    #[test]
    fn save_and_load_files() {
        let dir = std::env::temp_dir().join(format!("viscas-hist-{}", std::process::id()));
        let path = dir.join("sub/console_history");
        let entries = h(&["1 + 1", "sin(x)"]);
        append(&path, &entries).unwrap();
        assert_eq!(load(&path), entries);
        assert!(load(&dir.join("missing")).is_empty());
        // A second instance appends instead of clobbering.
        append(&path, &h(&["sin(x)", "2^10"])).unwrap();
        append(&path, &h(&["a := 1"])).unwrap();
        assert_eq!(load(&path), h(&["1 + 1", "sin(x)", "2^10", "a := 1"]));
        // No temp files left behind.
        let leftovers = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_skips_repeats_and_caps() {
        assert_eq!(merged(h(&["a"]), &h(&["a", " ", "b", "b"])), h(&["a", "b"]));
        let disk: Vec<String> = (0..HISTORY_LIMIT).map(|i| i.to_string()).collect();
        let all = merged(disk, &h(&["new"]));
        assert_eq!(all.len(), HISTORY_LIMIT);
        assert_eq!(all.last().map(String::as_str), Some("new"));
        assert_eq!(all[0], "1");
    }

    #[test]
    fn navigation_walks_and_restores_draft() {
        let hist = h(&["a := 1", "b := 2", "a + b"]);
        let mut nav = HistoryNav::default();
        assert_eq!(nav.down(&hist), None, "not browsing");
        assert_eq!(nav.up(&hist, "").as_deref(), Some("a + b"));
        assert_eq!(nav.up(&hist, "a + b").as_deref(), Some("b := 2"));
        assert_eq!(nav.up(&hist, "b := 2").as_deref(), Some("a := 1"));
        assert_eq!(nav.up(&hist, "a := 1"), None, "oldest reached");
        assert_eq!(nav.down(&hist).as_deref(), Some("b := 2"));
        assert_eq!(nav.down(&hist).as_deref(), Some("a + b"));
        assert_eq!(nav.down(&hist).as_deref(), Some(""), "draft restored");
        assert!(!nav.is_browsing());
    }

    #[test]
    fn navigation_filters_by_draft_prefix() {
        let hist = h(&["a := 1", "b := 2", "a + b"]);
        let mut nav = HistoryNav::default();
        assert_eq!(nav.up(&hist, "a").as_deref(), Some("a + b"));
        assert_eq!(nav.up(&hist, "a + b").as_deref(), Some("a := 1"));
        assert_eq!(nav.down(&hist).as_deref(), Some("a + b"));
        assert_eq!(nav.down(&hist).as_deref(), Some("a"));
        nav.reset();
        assert_eq!(nav.up(&hist, "zzz"), None);
        assert!(!nav.is_browsing());
    }
}
