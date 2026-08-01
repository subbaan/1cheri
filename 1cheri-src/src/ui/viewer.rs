use super::{build_overflow_menu, Shell, ViewerSession};
use crate::comments::{direct_replies, find_urls, parse_comment};
use crate::config::Config;
use crate::models::{ApiPost, ApiThread, MediaPost, MediaType, Thread};
use crate::player::{self, PlayerHandle};
use crate::{media_cache, net, storage, thumbnails};
use gtk4::prelude::*;
use gtk4::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as i64;
    format!("{}:{:02}", total / 60, total % 60)
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

/// `row.grab_focus()` moves keyboard focus but doesn't itself guarantee the
/// row is scrolled into view within its ScrolledWindow -- that auto-scroll
/// only happens for GTK's own internal keynav handling of Up/Down, not for
/// selections we make programmatically (resuming a thread, H/L navigation).
fn scroll_row_into_view(scroller: &gtk4::ScrolledWindow, list_box: &gtk4::ListBox, row: &gtk4::ListBoxRow) {
    // Bounds must be computed relative to `list_box` (the scrolled content
    // itself), not `scroller`: bounds-relative-to-scroller are already in
    // "currently visible viewport" space (post-scroll-transform), whereas
    // `vadjustment`'s value/clamp_page operate in content/document space --
    // using the former as if it were the latter silently scrolls to the
    // wrong position (or appears to do nothing from a value of 0).
    if let Some(bounds) = row.compute_bounds(list_box) {
        let y = f64::from(bounds.y());
        let height = f64::from(bounds.height());
        let adj = scroller.vadjustment();
        let page_size = adj.page_size();
        // Position the row a little above the vertical center, rather than
        // dead center or flush against an edge: leaves more room below to
        // preview upcoming thumbnails (the point of navigating at all) while
        // still keeping at least the last-seen one or two visible above.
        const ROW_POSITION_FRACTION: f64 = 0.38;
        let target = (y + height / 2.0) - page_size * ROW_POSITION_FRACTION;
        let target = target.clamp(adj.lower(), (adj.upper() - page_size).max(adj.lower()));
        adj.set_value(target);
    }
}

const GREENTEXT_COLOR: &str = "#789922";
const SAMPLE_THREAD_JSON: &str = include_str!("../../assets/sample-thread.json");

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn resolve_path(media_dir: &Path, source: &str) -> String {
    if source.starts_with("http://") || source.starts_with("https://") {
        source.to_string()
    } else {
        media_dir.join(source).to_string_lossy().into_owned()
    }
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn media_type_label(media_type: MediaType) -> &'static str {
    match media_type {
        MediaType::Video => "video",
        MediaType::AnimatedImage => "gif",
        MediaType::StaticImage => "image",
        MediaType::Unsupported => "?",
    }
}

/// Opens a real thread from the network: fetches it in the background, then
/// builds (or replaces) the viewer page. Errors are reported to stderr and
/// leave the catalogue on screen -- see the module-level note in net.rs
/// about known gaps versus the full §14 request manager.
pub fn open_thread(shell: &Rc<Shell>, board: &str, number: u64) {
    let shell = shell.clone();
    let board = board.to_string();
    net::fetch_thread(board.clone(), number, move |result| match result {
        Ok(api_thread) => {
            let thread = Thread::from_api(&board, api_thread);
            let video_count = thread.video_count();
            if let Err(e) = storage::update_video_count(&shell.db, &board, number, video_count) {
                eprintln!("[storage] failed to update video count: {e}");
            }
            let media_count = thread.reply_media_count();
            if let Err(e) = storage::update_last_seen_media_count(&shell.db, &board, number, media_count) {
                eprintln!("[storage] failed to update last_seen_media_count: {e}");
            }
            if let Err(e) = storage::set_last_opened(&shell.db, &board, number, now_unix()) {
                eprintln!("[storage] failed to set last_opened_at: {e}");
            }
            storage::set_app_state(&shell.db, "active_thread", &number.to_string()).ok();

            let resume_post = storage::get_last_media_post(&shell.db, &board, number).unwrap_or(None);
            build_page(&shell, thread, PathBuf::new(), resume_post);
        }
        Err(e) => {
            eprintln!("[viewer] failed to open /{board}/{number}: {e}");
            if matches!(e, net::ThreadFetchError::NotFound) {
                if let Err(err) = storage::delete_thread(&shell.db, &board, number) {
                    eprintln!("[storage] failed to remove vanished thread: {err}");
                }
                if let Some(reload) = shell.catalogue_needs_reload.borrow().as_ref() {
                    reload();
                }
            }
            if let Some(label) = shell.catalogue_status.borrow().as_ref() {
                label.set_text(&format!("Couldn't open thread #{number}: {e}"));
            }
        }
    });
}

/// Opens the bundled offline fixture thread directly, bypassing the network
/// entirely. Used by `--fixture` for local development/testing without
/// touching real board content.
pub fn open_fixture_thread(shell: &Rc<Shell>) {
    let api_thread: ApiThread = serde_json::from_str(SAMPLE_THREAD_JSON).expect("bundled sample-thread.json is valid");
    let thread = Thread::from_api("gif", api_thread);
    build_page(shell, thread, shell.fixture_media_dir.clone(), None);
}

#[derive(Clone)]
struct AppState {
    shell: Rc<Shell>,
    player: PlayerHandle,
    media: Rc<Vec<MediaPost>>,
    thread: Rc<Thread>,
    current_index: Rc<Cell<usize>>,
    list_box: gtk4::ListBox,
    thumbnail_scroller: gtk4::ScrolledWindow,
    media_info_label: gtk4::Label,
    comment_label: gtk4::Label,
    status_label: gtk4::Label,
    context_label: gtk4::Label,
    media_dir: Rc<PathBuf>,
}

impl AppState {
    fn config(&self) -> Rc<RefCell<Config>> {
        self.shell.config.clone()
    }

    fn apply_selection(&self, index: usize) {
        if self.media.is_empty() {
            return;
        }
        let index = index.min(self.media.len() - 1);
        self.current_index.set(index);
        let media = &self.media[index];

        let path = resolve_path(&self.media_dir, &media.source_path);
        let playable_path = media_cache::resolve_playable_path(&self.thread.board, &path);
        if let Some(p) = self.player.borrow().as_ref() {
            p.load(&playable_path);
            if !self.config().borrow().autoplay {
                p.set_paused(true);
            }
        }

        // Pre-warm the next item in the strip so navigating to it feels
        // instant, per project.md §14.3 -- only ever the one immediately
        // next, never the whole thread.
        if let Some(next) = self.media.get(index + 1) {
            let next_path = resolve_path(&self.media_dir, &next.source_path);
            media_cache::prefetch(&self.thread.board, &next_path);
        }

        storage::set_last_media_post(&self.shell.db, &self.thread.board, self.thread.op.no, media.post_number).ok();

        let full_name = format!("{}{}", media.original_filename, media.extension);
        let mut info = full_name.clone();
        if let (Some(w), Some(h)) = (media.width, media.height) {
            info.push_str(&format!("  ·  {w}×{h}"));
        }
        if let Some(size) = media.size_bytes {
            info.push_str(&format!("  ·  {}", format_size(size)));
        }
        self.media_info_label.set_text(&info);
        self.media_info_label.set_tooltip_text(Some(&full_name));

        self.update_comment_panel(media.post_number);
        self.shell.window.set_title(Some(&format!(
            "/{}/ {}  [{}/{}] #{} ({})",
            self.thread.board,
            self.thread.subject(),
            index + 1,
            self.media.len(),
            media.post_number,
            media_type_label(media.media_type),
        )));
        self.context_label.set_text(&format!(
            "/{}/ \u{b7} {}   #{}   [{}/{}]",
            self.thread.board,
            self.thread.subject(),
            media.post_number,
            index + 1,
            self.media.len(),
        ));
    }

    fn move_selection(&self, delta: isize) {
        if self.media.is_empty() {
            return;
        }
        let len = self.media.len() as isize;
        let current = self.current_index.get() as isize;
        let next = ((current + delta) % len + len) % len;
        if let Some(row) = self.list_box.row_at_index(next as i32) {
            self.list_box.select_row(Some(&row));
            // `select_row` alone updates the highlighted row but not GTK's
            // keyboard focus, so native Up/Down keynav would keep moving
            // relative to wherever focus actually was (e.g. row 0 after a
            // resume), independent of the row we just highlighted.
            // (Scrolling itself is handled by the `row-selected` handler,
            // which `select_row` triggers synchronously -- covers this and
            // native Up/Down keynav uniformly.)
            row.grab_focus();
        }
    }

    fn update_comment_panel(&self, post_number: u64) {
        let Some(post) = self.thread.post_by_number(post_number) else {
            self.comment_label.set_markup("");
            return;
        };

        let mut markup = format!("<b>Post #{}</b>\n", post.no);
        append_comment_markup(&mut markup, post);

        let replies = direct_replies(&self.thread, post.no);
        if !replies.is_empty() {
            markup.push_str("\n<b>Replies:</b>\n");
            for reply in replies {
                markup.push_str(&format!("<b>#{}:</b> ", reply.no));
                append_comment_markup(&mut markup, reply);
                markup.push('\n');
            }
        }

        self.comment_label.set_markup(&markup);
    }

    fn current_media(&self) -> Option<&MediaPost> {
        self.media.get(self.current_index.get())
    }

    /// Resolves the save path (per-thread dir, `{post}`/`{original_name}`
    /// templating, `_1`/`_2` collision-avoidance) and either copies from the
    /// local cache or downloads `media`, reporting the outcome via `on_done`
    /// instead of touching `status_label` directly -- shared by
    /// `save_current` (single item) and `save_all_media` (bulk), which each
    /// need their own status-message strategy around the same underlying
    /// per-item logic.
    fn save_media_item(&self, media: &MediaPost, on_done: impl FnOnce(Result<PathBuf, String>) + 'static) {
        let config = self.config();
        let config_ref = config.borrow();

        let thread_dir_name = format!("{}-{}", self.thread.op.no, slugify(&self.thread.subject()));
        let target_dir = config_ref
            .save_directory_expanded()
            .join(&self.thread.board)
            .join(thread_dir_name);

        let base_name = config_ref
            .filename_template
            .replace("{post}", &media.post_number.to_string())
            .replace("{original_name}", &media.original_filename);
        drop(config_ref);
        let mut target_name = format!("{base_name}{}", media.extension);
        let mut target_path = target_dir.join(&target_name);
        let mut suffix = 1;
        while target_path.exists() {
            target_name = format!("{base_name}_{suffix}{}", media.extension);
            target_path = target_dir.join(&target_name);
            suffix += 1;
        }

        let source = resolve_path(&self.media_dir, &media.source_path);

        if let Some(cached) = media_cache::cached_file_path(&self.thread.board, &source) {
            // Already downloaded (played, or prefetched as the next item) --
            // copy straight from the local cache instead of fetching again.
            let result = std::fs::create_dir_all(&target_dir)
                .and_then(|_| std::fs::copy(&cached, &target_path))
                .map(|_| target_path.clone())
                .map_err(|e| e.to_string());
            on_done(result);
        } else if source.starts_with("http://") || source.starts_with("https://") {
            if let Err(e) = std::fs::create_dir_all(&target_dir) {
                on_done(Err(e.to_string()));
                return;
            }
            let target_path_for_result = target_path.clone();
            net::download_to_file(source, target_path, move |result| {
                on_done(result.map(|()| target_path_for_result).map_err(|e| e));
            });
        } else {
            let result = std::fs::create_dir_all(&target_dir)
                .and_then(|_| std::fs::copy(&source, &target_path))
                .map(|_| target_path.clone())
                .map_err(|e| e.to_string());
            on_done(result);
        }
    }

    fn save_current(&self) {
        let Some(media) = self.current_media() else {
            return;
        };
        self.status_label.set_text("Saving...");
        let status_label = self.status_label.clone();
        self.save_media_item(media, move |result| {
            let message = match result {
                Ok(path) => format!("Saved to {}", path.display()),
                Err(e) => format!("Save failed: {e}"),
            };
            status_label.set_text(&message);
            let status_label = status_label.clone();
            glib::timeout_add_local_once(Duration::from_secs(4), move || status_label.set_text(""));
        });
    }

    /// Saves every media item in the thread (project.md §18.5's "bulk
    /// downloads" -- a manual, user-initiated whole-thread save). Runs up to
    /// `MAX_CONCURRENT` downloads at once, each new one staggered by
    /// `STAGGER` rather than fired immediately: `net::download_to_file` is
    /// unthrottled by design for the normal one-or-two-at-a-time case
    /// (current item + prefetch, per project.md §14), but a thread can have
    /// dozens of items, and 4chan doesn't document a rate limit for
    /// `i.4cdn.org` media the way it does for the `a.4cdn.org` API -- this
    /// errs conservative rather than assuming a burst of connections is
    /// fine. `button` is disabled for the duration as both a visual "this is
    /// running" cue and to prevent a second overlapping run.
    fn save_all_media(&self, button: &gtk4::Button) {
        let total = self.media.len();
        if total == 0 {
            return;
        }
        const MAX_CONCURRENT: usize = 2;
        button.set_sensitive(false);
        let queue: Rc<RefCell<VecDeque<MediaPost>>> = Rc::new(RefCell::new(self.media.iter().cloned().collect()));
        let completed = Rc::new(Cell::new(0usize));
        let failed = Rc::new(Cell::new(0usize));
        self.status_label.set_text(&format!("Saving media: 0/{total}..."));
        for i in 0..MAX_CONCURRENT.min(total) {
            // Staggers the initial workers' start times too, not just
            // dispatches after that -- otherwise the first MAX_CONCURRENT
            // connections would still all open in the same instant.
            self.schedule_next_download(&queue, &completed, &failed, total, button, Self::STAGGER * i as u32);
        }
    }

    /// A conservative gap between successive download dispatches -- see
    /// `save_all_media`'s doc comment for why this exists at all.
    const STAGGER: Duration = Duration::from_millis(400);

    /// Waits `delay`, then pops and saves the next queued item via
    /// `run_next_download`, if any remain.
    fn schedule_next_download(
        &self,
        queue: &Rc<RefCell<VecDeque<MediaPost>>>,
        completed: &Rc<Cell<usize>>,
        failed: &Rc<Cell<usize>>,
        total: usize,
        button: &gtk4::Button,
        delay: Duration,
    ) {
        let state = self.clone();
        let queue = queue.clone();
        let completed = completed.clone();
        let failed = failed.clone();
        let button = button.clone();
        glib::timeout_add_local_once(delay, move || {
            state.run_next_download(&queue, &completed, &failed, total, &button);
        });
    }

    /// Pops and saves the next queued item, if any, then schedules the next
    /// one (after `STAGGER`) on completion -- keeping up to `MAX_CONCURRENT`
    /// workers alive until the queue drains. Completion order across workers
    /// doesn't matter: this only counts how many have finished, never their
    /// position, so it stays correct regardless of which download lands
    /// first.
    fn run_next_download(
        &self,
        queue: &Rc<RefCell<VecDeque<MediaPost>>>,
        completed: &Rc<Cell<usize>>,
        failed: &Rc<Cell<usize>>,
        total: usize,
        button: &gtk4::Button,
    ) {
        let Some(media) = queue.borrow_mut().pop_front() else {
            return;
        };
        let state = self.clone();
        let queue = queue.clone();
        let completed = completed.clone();
        let failed = failed.clone();
        let button = button.clone();
        self.save_media_item(&media, move |result| {
            if result.is_err() {
                failed.set(failed.get() + 1);
            }
            let done = completed.get() + 1;
            completed.set(done);
            if done < total {
                state.status_label.set_text(&format!("Saving media: {done}/{total}..."));
                state.schedule_next_download(&queue, &completed, &failed, total, &button, Self::STAGGER);
            } else {
                let f = failed.get();
                let message = if f == 0 {
                    format!("Saved all {total} files.")
                } else {
                    format!("Saved {}/{total} files ({f} failed).", total - f)
                };
                state.status_label.set_text(&message);
                button.set_sensitive(true);
                let status_label = state.status_label.clone();
                glib::timeout_add_local_once(Duration::from_secs(5), move || status_label.set_text(""));
            }
        });
    }
}

fn append_comment_markup(markup: &mut String, post: &ApiPost) {
    let Some(com) = &post.com else {
        return;
    };
    for line in parse_comment(com) {
        let linkified = linkify_markup(&line.text);
        if line.greentext {
            markup.push_str(&format!("<span foreground=\"{GREENTEXT_COLOR}\">{linkified}</span>\n"));
        } else {
            markup.push_str(&format!("{linkified}\n"));
        }
    }
}

/// Escapes a line of comment text for Pango markup, wrapping any bare
/// http(s) URLs in `<a href>` so they render as clickable links.
fn linkify_markup(text: &str) -> String {
    let spans = find_urls(text);
    if spans.is_empty() {
        return glib::markup_escape_text(text).to_string();
    }
    let mut out = String::new();
    let mut last = 0;
    for (start, end) in spans {
        out.push_str(&glib::markup_escape_text(&text[last..start]));
        let escaped_url = glib::markup_escape_text(&text[start..end]);
        out.push_str(&format!("<a href=\"{escaped_url}\">{escaped_url}</a>"));
        last = end;
    }
    out.push_str(&glib::markup_escape_text(&text[last..]));
    out
}

fn build_thumbnail_widget(media_dir: &Path, board: &str, media: &MediaPost) -> gtk4::Widget {
    let thumb_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    thumb_container.set_size_request(120, 90);
    thumb_container.set_margin_top(4);
    thumb_container.set_margin_bottom(4);
    thumb_container.set_margin_start(4);
    thumb_container.set_margin_end(4);

    match &media.thumbnail_url {
        Some(url) => {
            let icon = gtk4::Image::from_icon_name(match media.media_type {
                MediaType::StaticImage => "image-x-generic-symbolic",
                _ => "video-x-generic-symbolic",
            });
            icon.set_pixel_size(48);
            thumb_container.append(&icon);
            thumbnails::load_into(&thumb_container, board, url, 120, 90);
        }
        None if media.media_type == MediaType::StaticImage => {
            // Offline fixture: no remote thumbnail exists, but the image
            // itself is a small local file, so just show that directly.
            let path = media_dir.join(&media.source_path);
            let picture = gtk4::Picture::for_filename(&path);
            picture.set_content_fit(gtk4::ContentFit::Cover);
            picture.set_size_request(120, 90);
            thumb_container.append(&picture);
        }
        None => {
            let icon = gtk4::Image::from_icon_name("video-x-generic-symbolic");
            icon.set_pixel_size(48);
            thumb_container.append(&icon);
        }
    }

    thumb_container.upcast()
}

fn build_page(shell: &Rc<Shell>, thread: Thread, media_dir: PathBuf, resume_post: Option<u64>) {
    // Tear down whatever viewer session (mpv instance + poll timers) may
    // already exist before creating a new one -- otherwise opening a
    // second thread in the same run leaks the first one forever.
    shell.teardown_viewer_session();

    let media = thread.media_posts();
    let board = thread.board.clone();

    // Persistent top bar, replacing the previous floating Back button (it
    // sat over the thumbnail strip rather than the video, and read as a
    // media control rather than app navigation). Gives the catalogue and
    // viewer a consistent "always has a top toolbar" feel.
    let back_button = gtk4::Button::with_label(&format!("\u{2039} /{board}/ catalog"));
    back_button.connect_clicked({
        let shell = shell.clone();
        move |_| shell.return_to_catalogue()
    });

    // Updated in AppState::apply_selection with board/subject/post
    // number/position -- the same information already computed there to set
    // the OS window title (easy to miss); this surfaces it on-screen.
    let context_label = gtk4::Label::new(None);
    context_label.set_xalign(0.0);
    context_label.set_hexpand(true);
    context_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);

    let replies_toggle = gtk4::ToggleButton::with_label("Replies");
    replies_toggle.set_active(true);

    let save_button = gtk4::Button::with_label("Save");
    let save_all_button = gtk4::Button::with_label("Save all media");

    let top_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    top_bar.set_margin_top(6);
    top_bar.set_margin_bottom(6);
    top_bar.set_margin_start(6);
    top_bar.set_margin_end(6);
    top_bar.append(&back_button);
    top_bar.append(&context_label);
    top_bar.append(&replies_toggle);
    top_bar.append(&save_button);
    top_bar.append(&build_overflow_menu(shell, &[save_all_button.clone()]));

    // Created here, ahead of the rest of the layout below, so attach_player
    // can report a playback error to it directly -- previously a file that
    // failed to load or play (corrupt, unsupported codec, dead link) failed
    // completely silently, with nothing on screen to explain a stuck player
    // (project.md §14.4).
    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_margin_start(6);
    status_label.set_margin_end(6);
    status_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    status_label.set_max_width_chars(1);
    status_label.set_hexpand(true);

    let gl_area = gtk4::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    let (player_handle, player_poll_source) = {
        let config = shell.config.borrow();
        let status_label = status_label.clone();
        player::attach_player(&gl_area, config.muted, config.volume, config.loop_video, move |err| {
            status_label.set_text(&format!("Playback error: {err}"));
            let status_label = status_label.clone();
            glib::timeout_add_local_once(Duration::from_secs(5), move || status_label.set_text(""));
        })
    };

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Browse);
    for m in media.iter() {
        list_box.append(&build_thumbnail_widget(&media_dir, &board, m));
    }
    let thumbnail_scroller = gtk4::ScrolledWindow::new();
    thumbnail_scroller.set_child(Some(&list_box));
    thumbnail_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);
    thumbnail_scroller.set_width_request(160);

    let media_info_label = gtk4::Label::new(None);
    media_info_label.set_xalign(0.0);
    media_info_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    media_info_label.set_margin_start(6);
    media_info_label.set_margin_end(6);
    media_info_label.set_margin_top(4);
    media_info_label.add_css_class("dim-label");

    let scrub_scale = gtk4::Scale::new(
        gtk4::Orientation::Horizontal,
        Some(&gtk4::Adjustment::new(0.0, 0.0, 1.0, 0.001, 0.01, 0.0)),
    );
    scrub_scale.set_draw_value(false);
    scrub_scale.set_hexpand(true);
    let time_label = gtk4::Label::new(Some("0:00 / 0:00"));
    time_label.set_width_chars(11);
    let scrub_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    scrub_row.set_margin_start(6);
    scrub_row.set_margin_end(6);
    scrub_row.set_margin_top(2);
    scrub_row.set_margin_bottom(2);
    scrub_row.append(&scrub_scale);
    scrub_row.append(&time_label);

    let comment_label = gtk4::Label::new(None);
    comment_label.set_use_markup(true);
    comment_label.set_selectable(true);
    comment_label.set_wrap(true);
    comment_label.set_xalign(0.0);
    comment_label.set_valign(gtk4::Align::Start);
    comment_label.set_margin_top(6);
    comment_label.set_margin_bottom(6);
    comment_label.set_margin_start(6);
    comment_label.set_margin_end(6);
    let comment_scroller = gtk4::ScrolledWindow::new();
    comment_scroller.set_child(Some(&comment_label));
    comment_scroller.set_height_request(140);
    comment_scroller.set_vexpand(false);

    replies_toggle.connect_toggled({
        let comment_scroller = comment_scroller.clone();
        move |toggle| comment_scroller.set_visible(toggle.is_active())
    });

    let right_pane = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    right_pane.append(&gl_area);
    right_pane.append(&media_info_label);
    right_pane.append(&scrub_row);
    right_pane.append(&comment_scroller);

    let content = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    content.append(&thumbnail_scroller);
    content.append(&right_pane);
    content.set_vexpand(true);

    let hint_label = gtk4::Label::new(Some(
        "H/L prev/next  \u{2190}/\u{2192} seek \u{00b1}5s  \u{2191}/\u{2193} list  Space pause  M mute  R restart  F fullscreen  S save  B/Esc back  Ctrl+Q quit",
    ));
    hint_label.set_xalign(1.0);
    hint_label.set_margin_start(6);
    hint_label.set_margin_end(6);
    let status_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    status_bar.append(&status_label);
    status_bar.set_hexpand(true);
    let status_bar_outer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    status_bar_outer.append(&status_bar);
    status_bar_outer.append(&hint_label);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    root.append(&top_bar);
    root.append(&content);
    root.append(&status_bar_outer);

    if let Some(old) = shell.stack.child_by_name("viewer") {
        shell.stack.remove(&old);
    }
    shell.stack.add_named(&root, Some("viewer"));
    shell.stack.set_visible_child_name("viewer");

    let state = AppState {
        shell: shell.clone(),
        player: player_handle,
        media: Rc::new(media),
        thread: Rc::new(thread),
        current_index: Rc::new(Cell::new(0)),
        list_box: list_box.clone(),
        thumbnail_scroller: thumbnail_scroller.clone(),
        media_info_label,
        comment_label,
        status_label,
        context_label,
        media_dir: Rc::new(media_dir),
    };

    {
        let state = state.clone();
        list_box.connect_row_selected(move |list_box, row| {
            if let Some(row) = row {
                state.apply_selection(row.index() as usize);
                // Covers native Up/Down keynav too, not just our own H/L
                // handling below: GTK's own focus-follows-keynav auto-scroll
                // only barely brings a newly-focused row into view (flush
                // against whichever edge it approached from -- the bottom,
                // moving downward), which left no room to see upcoming
                // thumbnails. Re-applying our own scroll positioning after
                // *every* selection change, regardless of what triggered it,
                // fixes that uniformly.
                scroll_row_into_view(&state.thumbnail_scroller, list_box, &row);
            }
        });
    }

    save_button.connect_clicked({
        let state = state.clone();
        move |_| state.save_current()
    });
    save_all_button.connect_clicked({
        let state = state.clone();
        let save_all_button = save_all_button.clone();
        move |_| state.save_all_media(&save_all_button)
    });

    let key_controller = gtk4::EventControllerKey::new();
    {
        let state = state.clone();
        key_controller.connect_key_pressed(move |_ctrl, key, _code, _modifiers| {
            match key {
                gdk::Key::h | gdk::Key::H => state.move_selection(-1),
                gdk::Key::l | gdk::Key::L => state.move_selection(1),
                gdk::Key::Left => {
                    if let Some(p) = state.player.borrow().as_ref() {
                        if let Some(pos) = p.position() {
                            p.seek_absolute((pos - 5.0).max(0.0));
                        }
                    }
                }
                gdk::Key::Right => {
                    if let Some(p) = state.player.borrow().as_ref() {
                        if let Some(pos) = p.position() {
                            p.seek_absolute(pos + 5.0);
                        }
                    }
                }
                gdk::Key::space => {
                    if let Some(p) = state.player.borrow().as_ref() {
                        let paused = p.is_paused();
                        p.set_paused(!paused);
                    }
                }
                gdk::Key::m | gdk::Key::M => {
                    if let Some(p) = state.player.borrow().as_ref() {
                        let muted = !p.is_muted();
                        p.set_muted(muted);
                        state.config().borrow_mut().muted = muted;
                        state.config().borrow().save();
                    }
                }
                gdk::Key::r | gdk::Key::R => {
                    if let Some(p) = state.player.borrow().as_ref() {
                        p.restart();
                    }
                }
                gdk::Key::f | gdk::Key::F | gdk::Key::F11 => {
                    if state.shell.window.is_fullscreen() {
                        state.shell.window.unfullscreen();
                    } else {
                        state.shell.window.fullscreen();
                    }
                }
                gdk::Key::s | gdk::Key::S => state.save_current(),
                gdk::Key::b | gdk::Key::B | gdk::Key::Escape => state.shell.return_to_catalogue(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    shell.window.add_controller(key_controller.clone());

    // Seeking: GtkRange's `change-value` fires only for genuine user
    // interaction (drag/click/keynav), never for our own programmatic
    // `set_value` calls below -- exactly what's needed to avoid the seek
    // handler and the position-refresh timer fighting each other.
    {
        let state = state.clone();
        scrub_scale.connect_change_value(move |_, _scroll, value| {
            if let Some(p) = state.player.borrow().as_ref() {
                if let Some(duration) = p.duration() {
                    p.seek_absolute(value.clamp(0.0, 1.0) * duration);
                }
            }
            glib::Propagation::Stop
        });
    }

    let scrub_poll_source = {
        let state = state.clone();
        let scrub_scale = scrub_scale.clone();
        let time_label = time_label.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if let Some(p) = state.player.borrow().as_ref() {
                if let (Some(pos), Some(duration)) = (p.position(), p.duration()) {
                    if duration > 0.0 {
                        scrub_scale.set_value((pos / duration).clamp(0.0, 1.0));
                    }
                    time_label.set_text(&format!("{} / {}", format_time(pos), format_time(duration)));
                }
            }
            glib::ControlFlow::Continue
        })
    };

    *shell.viewer_session.borrow_mut() = Some(ViewerSession {
        key_controller,
        gl_area: gl_area.clone(),
        player: state.player.clone(),
        player_poll_source: RefCell::new(Some(player_poll_source)),
        scrub_poll_source: RefCell::new(Some(scrub_poll_source)),
    });

    let initial_index = resume_post
        .and_then(|post| state.media.iter().position(|m| m.post_number == post))
        .unwrap_or(0);
    if let Some(row) = list_box.row_at_index(initial_index as i32) {
        list_box.select_row(Some(&row));
        // Without this, GTK's keyboard focus stays wherever it defaulted to
        // (row 0) even though the highlighted/selected row correctly jumps
        // to the resume position -- so native Up/Down keynav would start
        // moving relative to row 0, not the resumed row, on a resumed
        // thread specifically (a fresh thread starts at row 0 for both,
        // masking the bug there).
        row.grab_focus();
        // A single scroll-into-view isn't enough here: this page was just
        // added to the stack, so the strip hasn't been size-allocated yet
        // (hence deferring at all), AND thumbnails for the ~100+ rows above
        // the resume point are still arriving asynchronously over the next
        // couple of seconds, each one resizing its row from a placeholder
        // icon to a real image and shifting everything below it -- which
        // silently undoes an earlier scroll. Retrying a few times over 2.5s
        // rides that out; each call is a cheap no-op once nothing's moved.
        for delay_ms in [0, 150, 500, 1200, 2500] {
            let thumbnail_scroller = thumbnail_scroller.clone();
            let list_box = list_box.clone();
            let row = row.clone();
            glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
                scroll_row_into_view(&thumbnail_scroller, &list_box, &row);
            });
        }
    }
    // Call directly rather than relying solely on the `row-selected` signal:
    // GTK doesn't reliably re-emit it for a programmatic selection made this
    // early in a stack page's life (observed missing during real thread
    // opens, present during --fixture's simpler startup path). Idempotent,
    // so a redundant signal firing too causes no harm.
    state.apply_selection(initial_index);
}
