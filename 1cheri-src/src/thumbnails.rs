// Thumbnail disk cache, per project.md §14.2. Shared by the catalogue
// (thread rows) and the viewer (media strip).

use gtk4::prelude::*;
use std::path::PathBuf;

fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join(".cache/1cheri/thumbnails")
}

fn cached_path(board: &str, url: &str) -> PathBuf {
    let filename = url.rsplit('/').next().unwrap_or("thumb.jpg");
    cache_dir().join(board).join(filename)
}

fn show_picture(container: &gtk4::Box, path: &std::path::Path, width: i32, height: i32) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    let picture = gtk4::Picture::for_filename(path);
    picture.set_content_fit(gtk4::ContentFit::Cover);
    picture.set_size_request(width, height);
    container.append(&picture);
}

/// Shows the cached thumbnail for `url` inside `container` (replacing
/// whatever placeholder the caller put there), downloading it first if
/// needed. On download failure the placeholder is simply left in place,
/// per the §14.4 "thumbnail fails to load" fallback behaviour.
pub fn load_into(container: &gtk4::Box, board: &str, url: &str, width: i32, height: i32) {
    let path = cached_path(board, url);
    if path.exists() {
        show_picture(container, &path, width, height);
        return;
    }

    let container = container.clone();
    let path_for_download = path.clone();
    crate::net::download_to_file(url.to_string(), path, move |result| {
        if result.is_ok() {
            show_picture(&container, &path_for_download, width, height);
        }
    });
}
