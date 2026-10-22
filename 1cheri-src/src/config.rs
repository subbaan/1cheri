use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Persisted user settings (project.md §5.5, §9, §15). Only the subset the
/// v0.1.0 media-proof milestone actually reads or writes is included here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub muted: bool,
    pub volume: i64,
    pub loop_video: bool,
    pub autoplay: bool,
    pub save_directory: String,
    /// Whether saved media goes into a `board/thread-slug/` subfolder under
    /// `save_directory` (the original behaviour) or straight into
    /// `save_directory` itself with no subfolders. Filenames still include
    /// the post number by default, so flat saves rarely collide even across
    /// threads/boards -- see `ui::viewer::resolve_save_dir`.
    pub save_organize_by_thread: bool,
    pub filename_template: String,
    pub boards: Vec<String>,
    pub active_board: String,
    /// One of "bump_order", "newest", "most_replies", "most_images",
    /// "most_videos" -- see ui/catalogue.rs `sort_threads`.
    pub sort_threads_by: String,
    #[serde(default)]
    pub board_filters: Vec<BoardFilters>,
    /// Pin/hide words that apply to every board, unioned with each board's
    /// own `pin_words`/`hide_words` at match time (ui/catalogue.rs). Kept as
    /// flat top-level lists rather than a `board == "*"` sentinel row in
    /// `board_filters`, since that would need every lookup site to special-
    /// case it.
    #[serde(default)]
    pub global_pin_words: Vec<String>,
    #[serde(default)]
    pub global_hide_words: Vec<String>,
    /// Whether to reopen the last-viewed thread (and media position within
    /// it) on startup. Off by default, including for existing configs
    /// missing this field (via `Config::default()`'s value below) -- opt-in
    /// per the user's request, since always resuming into a thread you'd
    /// already backed out of before quitting felt surprising.
    #[serde(default)]
    pub resume_last_thread: bool,
    /// Last-used size of the Filters/Settings/Temporary-panel windows,
    /// remembered so they reopen at whatever size the user last resized
    /// them to instead of resetting to a fixed default every time (see
    /// `ui::remember_window_size`). Missing fields (e.g. an older
    /// config.toml) fall back to `Config::default()`'s values below via the
    /// struct-level `#[serde(default)]` above, same as every other field.
    pub filters_window_width: i32,
    pub filters_window_height: i32,
    pub settings_window_width: i32,
    pub settings_window_height: i32,
    pub temporary_window_width: i32,
    pub temporary_window_height: i32,
    pub board_directory_window_width: i32,
    pub board_directory_window_height: i32,
    /// How the Filters window orders words: "added" (list order, the
    /// default), "alpha", or "matches" (most live matches first).
    pub filters_sort: String,
}

/// Per-board pin/hide word (or phrase) lists, per project.md §11 plus
/// pinning. See filters.rs for how these are matched.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoardFilters {
    pub board: String,
    #[serde(default)]
    pub pin_words: Vec<String>,
    #[serde(default)]
    pub hide_words: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    Pin,
    Hide,
}

/// Which pin/hide lists a word lives in: one board's own, or the global
/// ones that apply to every board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterScope {
    Global,
    Board(String),
}

impl Default for Config {
    fn default() -> Self {
        Config {
            muted: true,
            volume: 70,
            loop_video: true,
            autoplay: true,
            save_directory: "~/Downloads/1cheri".to_string(),
            save_organize_by_thread: true,
            filename_template: "{post}_{original_name}".to_string(),
            // Worksafe by default -- /wsg/ ("Worksafe GIF") and /g/
            // (Technology) -- since a fresh install shouldn't silently drop
            // a new user onto an 18+ board without them having chosen that
            // themselves. /gif/ (the board this app was actually built
            // around, and genuinely NSFW) is a deliberate opt-in the user
            // adds themselves via Settings, not a default.
            boards: vec!["wsg".to_string(), "g".to_string()],
            active_board: "wsg".to_string(),
            sort_threads_by: "most_videos".to_string(),
            board_filters: Vec::new(),
            global_pin_words: Vec::new(),
            global_hide_words: Vec::new(),
            resume_last_thread: false,
            filters_window_width: 640,
            filters_window_height: 520,
            settings_window_width: 360,
            settings_window_height: 580,
            temporary_window_width: 360,
            temporary_window_height: 420,
            board_directory_window_width: 420,
            board_directory_window_height: 500,
            filters_sort: "added".to_string(),
        }
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME must be set"))
}

pub fn config_path() -> PathBuf {
    home_dir().join(".config/1cheri/config.toml")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!("[config] failed to parse {}: {e}, using defaults", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("[config] failed to create {}: {e}", parent.display());
                return;
            }
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = fs::write(&path, text) {
                    eprintln!("[config] failed to write {}: {e}", path.display());
                }
            }
            Err(e) => eprintln!("[config] failed to serialise config: {e}"),
        }
    }

    /// Expands a leading `~` in `save_directory` to $HOME.
    pub fn save_directory_expanded(&self) -> PathBuf {
        if let Some(rest) = self.save_directory.strip_prefix("~/") {
            home_dir().join(rest)
        } else {
            PathBuf::from(&self.save_directory)
        }
    }

    pub fn pin_words_for(&self, board: &str) -> &[String] {
        self.board_filters
            .iter()
            .find(|f| f.board == board)
            .map(|f| f.pin_words.as_slice())
            .unwrap_or(&[])
    }

    pub fn hide_words_for(&self, board: &str) -> &[String] {
        self.board_filters
            .iter()
            .find(|f| f.board == board)
            .map(|f| f.hide_words.as_slice())
            .unwrap_or(&[])
    }

    fn board_filters_mut(&mut self, board: &str) -> &mut BoardFilters {
        if let Some(pos) = self.board_filters.iter().position(|f| f.board == board) {
            &mut self.board_filters[pos]
        } else {
            self.board_filters.push(BoardFilters {
                board: board.to_string(),
                ..Default::default()
            });
            self.board_filters.last_mut().unwrap()
        }
    }

    pub fn global_pin_words(&self) -> &[String] {
        &self.global_pin_words
    }

    pub fn global_hide_words(&self) -> &[String] {
        &self.global_hide_words
    }

    /// Any of the four pin/hide lists, addressed by scope and kind instead
    /// of one accessor per list -- what the Filters editor needs to move or
    /// rename a word between lists generically.
    pub fn filter_words(&self, scope: &FilterScope, kind: FilterKind) -> &[String] {
        match (scope, kind) {
            (FilterScope::Global, FilterKind::Pin) => &self.global_pin_words,
            (FilterScope::Global, FilterKind::Hide) => &self.global_hide_words,
            (FilterScope::Board(board), FilterKind::Pin) => self.pin_words_for(board),
            (FilterScope::Board(board), FilterKind::Hide) => self.hide_words_for(board),
        }
    }

    fn filter_words_mut(&mut self, scope: &FilterScope, kind: FilterKind) -> &mut Vec<String> {
        match (scope, kind) {
            (FilterScope::Global, FilterKind::Pin) => &mut self.global_pin_words,
            (FilterScope::Global, FilterKind::Hide) => &mut self.global_hide_words,
            (FilterScope::Board(board), FilterKind::Pin) => &mut self.board_filters_mut(board).pin_words,
            (FilterScope::Board(board), FilterKind::Hide) => &mut self.board_filters_mut(board).hide_words,
        }
    }

    pub fn add_filter_word(&mut self, scope: &FilterScope, kind: FilterKind, word: String) {
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        let words = self.filter_words_mut(scope, kind);
        if !words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            words.push(word);
        }
    }

    pub fn remove_filter_word(&mut self, scope: &FilterScope, kind: FilterKind, word: &str) {
        self.filter_words_mut(scope, kind).retain(|w| w != word);
    }

    /// Replaces `old` with `new` in place, keeping its position in the list
    /// (and so its place in "Order added" sorting). If `new` duplicates
    /// another entry already in the list, `old` is just dropped.
    pub fn rename_filter_word(&mut self, scope: &FilterScope, kind: FilterKind, old: &str, new: String) {
        let new = new.trim().to_string();
        if new.is_empty() || new == old {
            return;
        }
        let words = self.filter_words_mut(scope, kind);
        let duplicate = words.iter().any(|w| w != old && w.eq_ignore_ascii_case(&new));
        if duplicate {
            words.retain(|w| w != old);
        } else if let Some(slot) = words.iter_mut().find(|w| *w == old) {
            *slot = new;
        }
    }

    /// Moves `word` from one list to another (pin <-> hide, board <->
    /// global). Appends to the destination unless it's already there.
    pub fn move_filter_word(
        &mut self,
        from: (&FilterScope, FilterKind),
        to: (&FilterScope, FilterKind),
        word: &str,
    ) {
        self.remove_filter_word(from.0, from.1, word);
        self.add_filter_word(to.0, to.1, word.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> FilterScope {
        FilterScope::Board("gif".to_string())
    }

    #[test]
    fn rename_keeps_position() {
        let mut c = Config::default();
        for w in ["a", "b", "c"] {
            c.add_filter_word(&board(), FilterKind::Hide, w.to_string());
        }
        c.rename_filter_word(&board(), FilterKind::Hide, "b", " bee ".to_string());
        assert_eq!(c.filter_words(&board(), FilterKind::Hide), ["a", "bee", "c"]);
    }

    #[test]
    fn rename_onto_existing_word_drops_old() {
        let mut c = Config::default();
        for w in ["a", "b"] {
            c.add_filter_word(&board(), FilterKind::Hide, w.to_string());
        }
        c.rename_filter_word(&board(), FilterKind::Hide, "b", "A".to_string());
        assert_eq!(c.filter_words(&board(), FilterKind::Hide), ["a"]);
    }

    #[test]
    fn rename_to_blank_is_ignored() {
        let mut c = Config::default();
        c.add_filter_word(&board(), FilterKind::Pin, "a".to_string());
        c.rename_filter_word(&board(), FilterKind::Pin, "a", "  ".to_string());
        assert_eq!(c.filter_words(&board(), FilterKind::Pin), ["a"]);
    }

    #[test]
    fn move_between_kinds_and_scopes() {
        let mut c = Config::default();
        c.add_filter_word(&board(), FilterKind::Hide, "x".to_string());
        c.move_filter_word((&board(), FilterKind::Hide), (&board(), FilterKind::Pin), "x");
        assert!(c.filter_words(&board(), FilterKind::Hide).is_empty());
        assert_eq!(c.filter_words(&board(), FilterKind::Pin), ["x"]);
        c.move_filter_word((&board(), FilterKind::Pin), (&FilterScope::Global, FilterKind::Pin), "x");
        assert!(c.filter_words(&board(), FilterKind::Pin).is_empty());
        assert_eq!(c.global_pin_words(), ["x"]);
    }

    #[test]
    fn move_onto_existing_word_does_not_duplicate() {
        let mut c = Config::default();
        c.add_filter_word(&board(), FilterKind::Hide, "x".to_string());
        c.add_filter_word(&FilterScope::Global, FilterKind::Hide, "X".to_string());
        c.move_filter_word((&board(), FilterKind::Hide), (&FilterScope::Global, FilterKind::Hide), "x");
        assert!(c.filter_words(&board(), FilterKind::Hide).is_empty());
        assert_eq!(c.global_hide_words(), ["X"]);
    }
}
