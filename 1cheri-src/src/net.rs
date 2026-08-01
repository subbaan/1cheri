// Centralised network access, per project.md §14. Every request runs on a
// spawned std::thread -- never the GTK main thread (§21) -- and results are
// delivered back via a channel polled through the same glib-timeout style
// already used for mpv in player.rs.
//
// Known simplifications versus the full §14 request manager: no conditional
// (If-Modified-Since) requests yet, and no retry/backoff. The global
// one-request-per-second budget IS enforced (via `throttle_global`); the
// per-board/thread ten-second refresh debounce is enforced by the caller
// (ui/catalogue.rs), since that needs UI-visible state (a status message)
// rather than being purely a network concern.

use crate::models::{ApiThread, CatalogPage};
use gtk4::glib;
use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const USER_AGENT: &str = concat!(
    "1cheri/",
    env!("CARGO_PKG_VERSION"),
    " (Rust; +https://github.com/subbaan/1cheri)"
);

fn global_rate_gate() -> &'static Mutex<Option<Instant>> {
    static GATE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

/// Blocks the calling (background) thread just long enough to keep requests
/// to 4chan's API to at most one per second (project.md §14).
fn throttle_global() {
    let gate = global_rate_gate();
    let mut last = gate.lock().unwrap();
    if let Some(prev) = *last {
        let elapsed = prev.elapsed();
        if elapsed < Duration::from_secs(1) {
            thread::sleep(Duration::from_secs(1) - elapsed);
        }
    }
    *last = Some(Instant::now());
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(10))
        .build()
}

fn poll_channel<T: Send + 'static, F: FnOnce(T) + 'static>(rx: mpsc::Receiver<T>, on_done: F) {
    let on_done = RefCell::new(Some(on_done));
    glib::timeout_add_local(Duration::from_millis(50), move || {
        match rx.try_recv() {
            Ok(value) => {
                if let Some(f) = on_done.borrow_mut().take() {
                    f(value);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

pub fn fetch_catalogue<F: FnOnce(Result<Vec<CatalogPage>, String>) + 'static>(board: String, on_done: F) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        throttle_global();
        let url = format!("https://a.4cdn.org/{board}/catalog.json");
        let result = agent()
            .get(&url)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|resp| resp.into_json::<Vec<CatalogPage>>().map_err(|e| e.to_string()));
        let _ = tx.send(result);
    });
    poll_channel(rx, on_done);
}

/// Downloads `url` to `target` on a background thread: used both for saving
/// a media file that was only ever streamed by the player, and for
/// thumbnails. Deliberately NOT rate-limited like `fetch_catalogue`/
/// `fetch_thread`: those hit `a.4cdn.org`'s JSON API, which project.md §14's
/// one-request-per-second budget is about; this hits `i.4cdn.org`, a plain
/// static-file CDN, and a catalogue page's dozen-plus thumbnails would
/// otherwise queue up behind that shared gate and starve out anything
/// requested after them (this is exactly what happened when thread-opening
/// silently stalled behind a page full of thumbnail fetches).
/// Downloads `url` to `target`, atomically: streams into a uniquely-named
/// sibling temp file and only renames it into place once fully complete.
/// Without this, anything that treats "the file exists" as "the file is
/// ready" -- mpv loading a prefetched video, a thumbnail load -- could see
/// (and try to play/display) a partial file while the download is still in
/// flight. A failed download is cleaned up rather than left behind: a
/// half-written file sitting at the final path would look permanently
/// "cached" to every future existence check, silently serving corrupt data
/// forever afterward.
pub fn download_to_file<F: FnOnce(Result<(), String>) + 'static>(url: String, target: std::path::PathBuf, on_done: F) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let file_name = target.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let tmp_target = target.with_file_name(format!("{file_name}.{nanos}.part"));
        let result = (|| -> Result<(), String> {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let resp = agent().get(&url).call().map_err(|e| e.to_string())?;
            let mut reader = resp.into_reader();
            let mut file = std::fs::File::create(&tmp_target).map_err(|e| e.to_string())?;
            std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
            drop(file);
            std::fs::rename(&tmp_target, &target).map_err(|e| e.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp_target);
        }
        let _ = tx.send(result);
    });
    poll_channel(rx, on_done);
}

/// Distinguishes a confirmed-gone thread (HTTP 404) from any other failure
/// (network error, timeout, malformed response). The caller needs this
/// distinction to decide whether to evict the thread from the local cache --
/// evicting on a transient network error would wrongly wipe a still-live
/// thread just because a request happened to fail.
pub enum ThreadFetchError {
    NotFound,
    Other(String),
}

impl std::fmt::Display for ThreadFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadFetchError::NotFound => write!(f, "thread not found (404)"),
            ThreadFetchError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub fn fetch_thread<F: FnOnce(Result<ApiThread, ThreadFetchError>) + 'static>(board: String, number: u64, on_done: F) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        throttle_global();
        let url = format!("https://a.4cdn.org/{board}/thread/{number}.json");
        let result = match agent().get(&url).call() {
            Ok(resp) => resp.into_json::<ApiThread>().map_err(|e| ThreadFetchError::Other(e.to_string())),
            Err(ureq::Error::Status(404, _)) => Err(ThreadFetchError::NotFound),
            Err(e) => Err(ThreadFetchError::Other(e.to_string())),
        };
        let _ = tx.send(result);
    });
    poll_channel(rx, on_done);
}
