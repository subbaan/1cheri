// Redirects the whole process's stdout/stderr to a log file, per project.md
// §18.5's "better diagnostics" item. This isn't just wrapping our own
// eprintln! calls: some of the noisiest terminal output comes from mpv/
// ffmpeg's hardware-decoder backend probing (e.g. "Cannot load
// libcuda.so.1"), which writes straight to the real stderr file descriptor
// and bypasses mpv's own "terminal=no" setting and any message-level
// filtering entirely (confirmed empirically -- see player.rs's comment on
// hwdec). A file-descriptor-level redirect is the only thing that actually
// catches it.

use std::fs::File;
use std::os::unix::io::IntoRawFd;
use std::path::PathBuf;

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME must be set"))
}

/// `~/.local/state/` (not `.cache`/`.config`/`.local/share`, which the rest
/// of the app already uses for other things) is the XDG-designated home for
/// exactly this: logs and other state that's useful to keep around but
/// isn't config and isn't precious data.
pub fn log_path() -> PathBuf {
    home_dir().join(".local/state/1cheri/1cheri.log")
}

/// Truncates and reopens the log file fresh on every launch -- this
/// session's output only, not an ever-growing history -- and redirects both
/// stdout and stderr to it at the file-descriptor level. Falls back to
/// leaving output on the terminal (with a warning explaining why) if the log
/// file can't be created, rather than silently discarding everything.
pub fn redirect_stdio_to_log_file() {
    let path = log_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[logging] failed to create {}: {e}, leaving output on the terminal", parent.display());
            return;
        }
    }
    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[logging] failed to create {}: {e}, leaving output on the terminal", path.display());
            return;
        }
    };
    let fd = file.into_raw_fd();
    unsafe {
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
        // dup2 makes fds 1/2 independently reference the same underlying
        // open file description, so this doesn't affect them once it's
        // closed -- it's only needed for the initial handoff.
        libc::close(fd);
    }
}
