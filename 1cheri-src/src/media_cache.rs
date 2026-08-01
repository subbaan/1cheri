// Local media cache: prefetches the next item in the thumbnail strip and
// serves already-cached files instead of re-streaming over the network, per
// project.md §14.3. Deliberately simple: no active eviction/byte-budget
// enforcement in this pass (previous media stays retained "for free" simply
// because nothing ever deletes it) -- see ui/viewer.rs's apply_selection for
// how this is wired in. Only ever downloads the *next* item, never a whole
// thread, per §14.3's explicit "do not download an entire thread
// automatically" constraint.

use std::path::PathBuf;

fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join(".cache/1cheri/media")
}

fn cached_path(board: &str, url: &str) -> PathBuf {
    let filename = url.rsplit('/').next().unwrap_or("media");
    cache_dir().join(board).join(filename)
}

fn is_remote(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

/// The path mpv should actually load: an already-downloaded local copy if
/// present (instant, no streaming delay), otherwise the original URL
/// unchanged -- mpv streams that directly, exactly as it did before this
/// cache existed. Local fixture paths (never `http`) pass through untouched.
pub fn resolve_playable_path(board: &str, source: &str) -> String {
    if !is_remote(source) {
        return source.to_string();
    }
    let path = cached_path(board, source);
    if path.exists() {
        path.to_string_lossy().into_owned()
    } else {
        source.to_string()
    }
}

/// The cached file for `source`, if one already exists -- lets `save`
/// copy a local file instead of downloading again.
pub fn cached_file_path(board: &str, source: &str) -> Option<PathBuf> {
    if !is_remote(source) {
        return None;
    }
    let path = cached_path(board, source);
    path.exists().then_some(path)
}

/// Fire-and-forget background download of `source`, if it's remote and not
/// already cached. Used to pre-warm the *next* item in the thumbnail strip
/// so navigating to it feels instant.
pub fn prefetch(board: &str, source: &str) {
    if !is_remote(source) {
        return;
    }
    let path = cached_path(board, source);
    if path.exists() {
        return;
    }
    let board_for_log = board.to_string();
    crate::net::download_to_file(source.to_string(), path, move |result| {
        if let Err(e) = result {
            eprintln!("[media_cache] prefetch failed for /{board_for_log}/: {e}");
        }
    });
}
