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

impl Default for Config {
    fn default() -> Self {
        Config {
            muted: true,
            volume: 70,
            loop_video: true,
            autoplay: true,
            save_directory: "~/Downloads/1cheri".to_string(),
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
            filters_window_width: 420,
            filters_window_height: 620,
            settings_window_width: 360,
            settings_window_height: 580,
            temporary_window_width: 360,
            temporary_window_height: 420,
            board_directory_window_width: 420,
            board_directory_window_height: 500,
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

    pub fn add_pin_word(&mut self, board: &str, word: String) {
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        let filters = self.board_filters_mut(board);
        if !filters.pin_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            filters.pin_words.push(word);
        }
    }

    pub fn add_hide_word(&mut self, board: &str, word: String) {
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        let filters = self.board_filters_mut(board);
        if !filters.hide_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            filters.hide_words.push(word);
        }
    }

    pub fn remove_pin_word(&mut self, board: &str, word: &str) {
        self.board_filters_mut(board).pin_words.retain(|w| w != word);
    }

    pub fn remove_hide_word(&mut self, board: &str, word: &str) {
        self.board_filters_mut(board).hide_words.retain(|w| w != word);
    }

    pub fn global_pin_words(&self) -> &[String] {
        &self.global_pin_words
    }

    pub fn global_hide_words(&self) -> &[String] {
        &self.global_hide_words
    }

    pub fn add_global_pin_word(&mut self, word: String) {
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        if !self.global_pin_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            self.global_pin_words.push(word);
        }
    }

    pub fn add_global_hide_word(&mut self, word: String) {
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        if !self.global_hide_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
            self.global_hide_words.push(word);
        }
    }

    pub fn remove_global_pin_word(&mut self, word: &str) {
        self.global_pin_words.retain(|w| w != word);
    }

    pub fn remove_global_hide_word(&mut self, word: &str) {
        self.global_hide_words.retain(|w| w != word);
    }
}
