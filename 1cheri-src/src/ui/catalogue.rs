use super::viewer;
use super::{build_editable_list_widget, build_overflow_menu, Shell};
use crate::filters;
use crate::models::{TempFlagKind, ThreadSummary};
use crate::{net, storage, thumbnails};
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

const REFRESH_DEBOUNCE: Duration = Duration::from_secs(10);
const SORT_LABELS: [&str; 5] = ["Most videos", "Most images", "Most replies", "Newest thread", "Recent activity"];
const SORT_KEYS: [&str; 5] = ["most_videos", "most_images", "most_replies", "newest_thread", "recent_activity"];

fn sort_threads(threads: &mut [ThreadSummary], mode: &str) {
    match mode {
        "most_images" => threads.sort_by(|a, b| b.images.cmp(&a.images)),
        "most_replies" => threads.sort_by(|a, b| b.replies.cmp(&a.replies)),
        "newest_thread" => threads.sort_by(|a, b| b.number.cmp(&a.number)),
        "recent_activity" => threads.sort_by(|a, b| b.last_modified.cmp(&a.last_modified)),
        // "most_videos" and unknown modes: unopened threads (video_count ==
        // None) sort as 0 -- see models.rs ThreadSummary doc comment on why
        // an accurate count isn't known until a thread has been opened.
        _ => threads.sort_by(|a, b| b.video_count.unwrap_or(0).cmp(&a.video_count.unwrap_or(0))),
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_relative(unix_time: i64) -> String {
    let now = now_unix();
    let delta = (now - unix_time).max(0);
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

/// Wraps `content` in an explicit ListBoxRow so the `.pinned-row` CSS class
/// can be applied to the actual `row`-named GTK CSS node. `list_box.append`
/// auto-wraps a bare child widget in a GtkListBoxRow too, but that wrapper
/// isn't reachable to style afterwards -- adding the class to our own inner
/// content Box instead (as this used to do) silently never matched the
/// `row.pinned-row` selector, since a plain Box's CSS node name is "box",
/// not "row".
fn wrap_in_row(content: &gtk4::Widget, pinned: bool) -> gtk4::ListBoxRow {
    let list_row = gtk4::ListBoxRow::new();
    list_row.set_child(Some(content));
    if pinned {
        list_row.add_css_class("pinned-row");
    }
    list_row
}

fn build_row(summary: &ThreadSummary, pinned: bool, hidden_reasons: Option<&[String]>) -> gtk4::Widget {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    row.set_margin_start(6);
    row.set_margin_end(6);

    let thumb_container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    thumb_container.set_size_request(64, 64);
    let icon = gtk4::Image::from_icon_name("image-x-generic-symbolic");
    icon.set_pixel_size(48);
    thumb_container.append(&icon);
    if let Some(url) = &summary.thumbnail_url {
        // Falls back to the placeholder icon above on download failure,
        // per §14.4's "thumbnail fails to load" behaviour.
        thumbnails::load_into(&thumb_container, &summary.board, url, 64, 64);
    }
    row.append(&thumb_container);

    let text_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    text_box.set_hexpand(true);

    let subject = summary.subject.clone().unwrap_or_else(|| format!("Thread #{}", summary.number));
    let title_label = gtk4::Label::new(None);
    // A text badge, not just the row's colour tint, so a pinned thread is
    // unambiguous regardless of how easy the tint is to notice at a glance.
    // Plain text rather than a pin/star glyph: this system (and possibly the
    // user's) has no emoji or symbol font installed, so U+1F4CC/U+2605 etc.
    // render as nothing at all -- verified via fontTools against the
    // installed Noto Sans, which only covers basic Latin/punctuation.
    let title_markup = if pinned {
        format!(
            "<span foreground=\"#d4a017\"><b>PIN</b></span> <b>{}</b>",
            glib::markup_escape_text(&subject)
        )
    } else {
        format!("<b>{}</b>", glib::markup_escape_text(&subject))
    };
    title_label.set_markup(&title_markup);
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    text_box.append(&title_label);

    if !summary.comment_preview.is_empty() {
        let preview_label = gtk4::Label::new(Some(&summary.comment_preview));
        preview_label.set_xalign(0.0);
        preview_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        preview_label.add_css_class("dim-label");
        text_box.append(&preview_label);
    }

    let video_part = summary
        .video_count
        .map(|v| format!(" · {v} videos"))
        .unwrap_or_default();
    // How many media posts have appeared since the thread was last actually
    // opened (not just seen in the catalogue) -- lets the user tell at a
    // glance whether a previously-viewed thread is worth reopening, without
    // full per-item watched-state tracking.
    let new_part = match summary.last_seen_media_count {
        Some(known) if summary.images > known => {
            format!(
                "  <span foreground=\"#8bc34a\"><b>+{} new</b></span>",
                summary.images - known
            )
        }
        _ => String::new(),
    };
    let meta = format!(
        "#{} · {} replies · {} files{} · {}{}",
        summary.number,
        summary.replies,
        summary.images,
        video_part,
        format_relative(summary.last_modified),
        new_part,
    );
    let meta_label = gtk4::Label::new(None);
    meta_label.set_use_markup(true);
    meta_label.set_markup(&meta);
    meta_label.set_xalign(0.0);
    meta_label.add_css_class("dim-label");
    text_box.append(&meta_label);

    if let Some(reasons) = hidden_reasons {
        // project.md §11.6's "Hidden by: ... / Matched because ..." pattern,
        // computed live rather than persisted (see filters.rs doc comment).
        let reason_label = gtk4::Label::new(Some(&format!("Hidden by: {}", reasons.join(", "))));
        reason_label.set_xalign(0.0);
        reason_label.add_css_class("dim-label");
        text_box.append(&reason_label);
    }

    row.append(&text_box);
    row.upcast()
}

/// Wires a right-click popover onto a thread row offering "Pin matching this
/// subject" / "Hide matching this subject" -- project.md §11.5's quick-hide
/// sketch, extended to pinning. Adds the thread's full subject as a phrase;
/// it can be trimmed to something more general afterwards in the Filters
/// editor.
fn attach_quick_actions(
    row_widget: &gtk4::Widget,
    shell: &Rc<Shell>,
    current_board: &Rc<RefCell<String>>,
    summary: &ThreadSummary,
    trigger_render: &Rc<dyn Fn()>,
    reload_and_render: &Rc<dyn Fn()>,
    row_popovers: &Rc<RefCell<Vec<gtk4::Popover>>>,
) {
    let subject = summary.subject.clone().unwrap_or_else(|| format!("Thread #{}", summary.number));

    let popover = gtk4::Popover::new();
    popover.set_parent(row_widget);
    popover.set_has_arrow(true);
    // `set_parent` doesn't give the row an owning reference the way
    // `Box::append` would -- without tracking these and unparenting them
    // before the row itself is discarded on the next render, GTK warns
    // "Finalizing GtkButton/GtkBox, but it still has children left:
    // GtkPopover" every time the catalogue re-renders (search, sort,
    // filter, board switch all rebuild every row from scratch).
    row_popovers.borrow_mut().push(popover.clone());

    let popover_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let pin_button = gtk4::Button::with_label("Pin matching this subject");
    let hide_button = gtk4::Button::with_label("Hide matching this subject");
    // Temporary, auto-expiring per-thread flags, distinct from the
    // permanent word-based actions above: for the "this thread doesn't
    // match any word but I want it pinned/hidden for a while" case. A
    // temp-hide takes precedence over even a pin-word match (see the
    // partitioning order in build_page's render_impl), for the situational
    // "no, I want *this* thread hidden regardless" case.
    let is_temp_pinned = matches!(summary.temp_flag, Some(ref f) if f.kind == TempFlagKind::Pin);
    let is_temp_hidden = matches!(summary.temp_flag, Some(ref f) if f.kind == TempFlagKind::Hide);
    let temp_pin_button =
        gtk4::Button::with_label(if is_temp_pinned { "Un-pin (temporary)" } else { "Temporarily pin thread" });
    let temp_hide_button =
        gtk4::Button::with_label(if is_temp_hidden { "Un-hide (temporary)" } else { "Temporarily hide thread" });
    popover_box.append(&pin_button);
    popover_box.append(&hide_button);
    popover_box.append(&temp_pin_button);
    popover_box.append(&temp_hide_button);
    popover.set_child(Some(&popover_box));

    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let subject = subject.clone();
        let trigger_render = trigger_render.clone();
        let popover = popover.clone();
        pin_button.connect_clicked(move |_| {
            let board = current_board.borrow().clone();
            shell.config.borrow_mut().add_pin_word(&board, subject.clone());
            shell.config.borrow().save();
            popover.popdown();
            trigger_render();
        });
    }
    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let trigger_render = trigger_render.clone();
        let popover = popover.clone();
        hide_button.connect_clicked(move |_| {
            let board = current_board.borrow().clone();
            shell.config.borrow_mut().add_hide_word(&board, subject.clone());
            shell.config.borrow().save();
            popover.popdown();
            trigger_render();
        });
    }
    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let reload_and_render = reload_and_render.clone();
        let popover = popover.clone();
        let number = summary.number;
        temp_pin_button.connect_clicked(move |_| {
            let board = current_board.borrow().clone();
            let result = if is_temp_pinned {
                storage::clear_temp_flag(&shell.db, &board, number)
            } else {
                storage::set_temp_flag(&shell.db, &board, number, "pin")
            };
            if let Err(e) = result {
                eprintln!("[storage] failed to set temp pin flag: {e}");
            }
            popover.popdown();
            reload_and_render();
        });
    }
    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let reload_and_render = reload_and_render.clone();
        let popover = popover.clone();
        let number = summary.number;
        temp_hide_button.connect_clicked(move |_| {
            let board = current_board.borrow().clone();
            let result = if is_temp_hidden {
                storage::clear_temp_flag(&shell.db, &board, number)
            } else {
                storage::set_temp_flag(&shell.db, &board, number, "hide")
            };
            if let Err(e) = result {
                eprintln!("[storage] failed to set temp hide flag: {e}");
            }
            popover.popdown();
            reload_and_render();
        });
    }

    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3); // secondary/right mouse button
    {
        let popover = popover.clone();
        gesture.connect_pressed(move |_, _, x, y| {
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    row_widget.add_controller(gesture);
}

fn open_filters_editor(shell: &Rc<Shell>, current_board: &Rc<RefCell<String>>, trigger_render: &Rc<dyn Fn()>) {
    let board = current_board.borrow().clone();
    let window = gtk4::Window::builder()
        .title(format!("Filters — /{board}/"))
        .transient_for(&shell.window)
        .default_width(420)
        .default_height(620)
        .build();

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let global_hint = gtk4::Label::new(Some("Applies to every board"));
    global_hint.set_xalign(0.0);
    global_hint.add_css_class("dim-label");

    let global_pin_words_widget = {
        let shell = shell.clone();
        let shell_add = shell.clone();
        let shell_remove = shell.clone();
        build_editable_list_widget(
            "Global pin words",
            "word or phrase",
            true,
            move || shell.config.borrow().global_pin_words().to_vec(),
            move |word| {
                shell_add.config.borrow_mut().add_global_pin_word(word);
                shell_add.config.borrow().save();
            },
            move |word| {
                shell_remove.config.borrow_mut().remove_global_pin_word(word);
                shell_remove.config.borrow().save();
            },
            trigger_render.clone(),
        )
    };
    let global_hide_words_widget = {
        let shell = shell.clone();
        let shell_add = shell.clone();
        let shell_remove = shell.clone();
        build_editable_list_widget(
            "Global hide words",
            "word or phrase",
            true,
            move || shell.config.borrow().global_hide_words().to_vec(),
            move |word| {
                shell_add.config.borrow_mut().add_global_hide_word(word);
                shell_add.config.borrow().save();
            },
            move |word| {
                shell_remove.config.borrow_mut().remove_global_hide_word(word);
                shell_remove.config.borrow().save();
            },
            trigger_render.clone(),
        )
    };

    let pin_words_widget = {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let shell_add = shell.clone();
        let current_board_add = current_board.clone();
        let shell_remove = shell.clone();
        let current_board_remove = current_board.clone();
        build_editable_list_widget(
            "Pin words",
            "word or phrase",
            true,
            move || shell.config.borrow().pin_words_for(&current_board.borrow()).to_vec(),
            move |word| {
                let board = current_board_add.borrow().clone();
                shell_add.config.borrow_mut().add_pin_word(&board, word);
                shell_add.config.borrow().save();
            },
            move |word| {
                let board = current_board_remove.borrow().clone();
                shell_remove.config.borrow_mut().remove_pin_word(&board, word);
                shell_remove.config.borrow().save();
            },
            trigger_render.clone(),
        )
    };
    let hide_words_widget = {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let shell_add = shell.clone();
        let current_board_add = current_board.clone();
        let shell_remove = shell.clone();
        let current_board_remove = current_board.clone();
        build_editable_list_widget(
            "Hide words",
            "word or phrase",
            true,
            move || shell.config.borrow().hide_words_for(&current_board.borrow()).to_vec(),
            move |word| {
                let board = current_board_add.borrow().clone();
                shell_add.config.borrow_mut().add_hide_word(&board, word);
                shell_add.config.borrow().save();
            },
            move |word| {
                let board = current_board_remove.borrow().clone();
                shell_remove.config.borrow_mut().remove_hide_word(&board, word);
                shell_remove.config.borrow().save();
            },
            trigger_render.clone(),
        )
    };

    root.append(&global_hint);
    root.append(&global_pin_words_widget);
    root.append(&global_hide_words_widget);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    root.append(&pin_words_widget);
    root.append(&hide_words_widget);

    let scroller = gtk4::ScrolledWindow::new();
    scroller.set_child(Some(&root));
    scroller.set_vexpand(true);

    window.set_child(Some(&scroller));
    window.present();
}

/// Lists every thread on the current board that's currently temporarily
/// pinned or hidden (project.md-adjacent quality-of-life addition, not from
/// the original spec): each row shows the subject, the flag kind, and how
/// long ago it was set, with a "Remove" button. Self-rebuilds on removal
/// via the same `Rc<RefCell<Option<Rc<dyn Fn()>>>>` cell pattern used for
/// `rebuild_tabs_cell` below, since entries can disappear while this window
/// is open.
fn open_temp_flags_panel(shell: &Rc<Shell>, current_board: &Rc<RefCell<String>>, reload_and_render: &Rc<dyn Fn()>) {
    let board = current_board.borrow().clone();
    let window = gtk4::Window::builder()
        .title(format!("Temporary pins/hides — /{board}/"))
        .transient_for(&shell.window)
        .default_width(360)
        .default_height(420)
        .build();

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    let scroller = gtk4::ScrolledWindow::new();
    scroller.set_child(Some(&list_box));
    scroller.set_vexpand(true);
    root.append(&scroller);

    let rebuild_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let reload_and_render = reload_and_render.clone();
        let list_box = list_box.clone();
        let rebuild_cell_for_rows = rebuild_cell.clone();
        let rebuild = move || {
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            let board = current_board.borrow().clone();
            let entries = storage::load_temp_flags(&shell.db, &board).unwrap_or_default();
            if entries.is_empty() {
                let empty_label = gtk4::Label::new(Some("Nothing temporarily pinned or hidden."));
                empty_label.add_css_class("dim-label");
                empty_label.set_margin_top(12);
                list_box.append(&empty_label);
            }
            for entry in entries {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                row.set_margin_top(4);
                row.set_margin_bottom(4);

                let kind_label = if entry.kind == "pin" { "Pinned" } else { "Hidden" };
                let elapsed = now_unix() - entry.flagged_at;
                let remaining_days = ((storage::TEMP_FLAG_TTL_SECS - elapsed).max(0)) / 86400;
                // Expiry before the subject, not after: with ellipsize on
                // the end, a long subject would otherwise swallow "expires
                // in Nd" -- the one piece of info this panel most needs to
                // surface.
                let text = gtk4::Label::new(Some(&format!(
                    "{kind_label} · expires in {remaining_days}d · {}",
                    entry.subject
                )));
                text.set_xalign(0.0);
                text.set_hexpand(true);
                text.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                row.append(&text);

                let remove_button = gtk4::Button::with_label("Remove");
                {
                    let shell = shell.clone();
                    let board = board.clone();
                    let number = entry.number;
                    let reload_and_render = reload_and_render.clone();
                    let rebuild_cell = rebuild_cell_for_rows.clone();
                    remove_button.connect_clicked(move |_| {
                        if let Err(e) = storage::clear_temp_flag(&shell.db, &board, number) {
                            eprintln!("[storage] failed to clear temp flag: {e}");
                        }
                        reload_and_render();
                        if let Some(f) = rebuild_cell.borrow().as_ref() {
                            f();
                        }
                    });
                }
                row.append(&remove_button);

                list_box.append(&row);
            }
        };
        *rebuild_cell.borrow_mut() = Some(Rc::new(rebuild));
    }
    if let Some(f) = rebuild_cell.borrow().as_ref() {
        f();
    }

    window.set_child(Some(&root));
    window.present();
}

pub fn build_page(shell: &Rc<Shell>, initial_board: String) {
    let tabs_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);

    let search_entry = gtk4::Entry::new();
    search_entry.set_placeholder_text(Some("Search threads..."));
    search_entry.set_hexpand(true);

    // `/` jumps straight into the search box, a common convention on Linux
    // (browsers, mutt, less...). Attached once here, for the lifetime of the
    // window -- unlike the viewer's key controller, the catalogue page is
    // built once at startup and never torn down (see ui/mod.rs::build_window),
    // so there's no session to scope this to. Since it stays on the window
    // even while the viewer is showing (GTK delivers events to all of a
    // widget's controllers regardless of which stack child is visible), it
    // must check the catalogue is actually the visible page before grabbing
    // focus -- otherwise pressing `/` mid-video would silently steal
    // keyboard focus away from the viewer's own bindings.
    let search_focus_controller = gtk4::EventControllerKey::new();
    {
        let shell = shell.clone();
        let search_entry = search_entry.clone();
        search_focus_controller.connect_key_pressed(move |_ctrl, key, _code, _modifiers| {
            if key == gdk::Key::slash
                && shell.stack.visible_child_name().as_deref() == Some("catalogue")
                && !search_entry.has_focus()
            {
                search_entry.grab_focus();
                search_entry.select_region(0, -1);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    shell.window.add_controller(search_focus_controller);

    let sort_model = gtk4::StringList::new(&SORT_LABELS);
    let sort_dropdown = gtk4::DropDown::new(Some(sort_model), gtk4::Expression::NONE);
    let initial_sort_index = SORT_KEYS
        .iter()
        .position(|k| *k == shell.config.borrow().sort_threads_by)
        .unwrap_or(0);
    sort_dropdown.set_selected(initial_sort_index as u32);

    let filters_button = gtk4::Button::with_label("Filters");
    let temporary_button = gtk4::Button::with_label("Temporary");
    let hidden_button = gtk4::ToggleButton::with_label("Hidden (0)");
    let refresh_button = gtk4::Button::with_label("Refresh");

    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    toolbar.append(&tabs_box);
    toolbar.append(&search_entry);
    toolbar.append(&sort_dropdown);
    toolbar.append(&filters_button);
    toolbar.append(&temporary_button);
    toolbar.append(&hidden_button);
    toolbar.append(&refresh_button);
    toolbar.append(&build_overflow_menu(shell, &[]));
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Browse);
    list_box.set_activate_on_single_click(true);
    let list_scroller = gtk4::ScrolledWindow::new();
    list_scroller.set_child(Some(&list_box));
    list_scroller.set_vexpand(true);

    let status_label = gtk4::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_margin_start(6);
    status_label.set_margin_end(6);
    status_label.set_margin_bottom(4);
    status_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    status_label.set_max_width_chars(1);
    status_label.set_hexpand(true);
    // Exposed on Shell so viewer::open_thread can report a fetch failure
    // here -- opening a thread that 404s or can't be reached used to fail
    // completely silently (project.md §14.4).
    *shell.catalogue_status.borrow_mut() = Some(status_label.clone());

    let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    page.append(&toolbar);
    page.append(&list_scroller);
    page.append(&status_label);

    shell.stack.add_named(&page, Some("catalogue"));

    let summaries: Rc<RefCell<Vec<ThreadSummary>>> = Rc::new(RefCell::new(Vec::new()));
    // Exactly what's currently appended to `list_box`, in row order: pinning,
    // hiding, and search filtering mean this is no longer the same order (or
    // the same set) as `summaries`, so row-activation needs its own
    // index-to-thread mapping.
    let displayed: Rc<RefCell<Vec<ThreadSummary>>> = Rc::new(RefCell::new(Vec::new()));
    // Every row-level quick-actions Popover from the current render pass,
    // unparented and cleared at the start of the next one (see
    // attach_quick_actions).
    let row_popovers: Rc<RefCell<Vec<gtk4::Popover>>> = Rc::new(RefCell::new(Vec::new()));
    let last_fetch: Rc<RefCell<HashMap<String, Instant>>> = Rc::new(RefCell::new(HashMap::new()));
    let current_board: Rc<RefCell<String>> = Rc::new(RefCell::new(initial_board.clone()));
    let current_sort: Rc<RefCell<String>> = Rc::new(RefCell::new(SORT_KEYS[initial_sort_index].to_string()));
    let search_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    // Tracks which board the list was last rendered for, so render_impl can
    // tell a same-board re-render (pin/hide add, search, sort, temp-flag
    // change -- scroll position should be preserved) apart from an actual
    // board switch (scroll should reset to the top; the old position
    // belongs to different content).
    let last_rendered_board: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // `render` needs to be callable from inside the row widgets it builds
    // (the right-click quick-add actions and the Filters editor both need to
    // trigger a re-render after mutating config) -- a direct
    // `let render = Rc::new(move || { ... render ... })` can't reference
    // itself while being constructed, so `trigger_render` is a stable,
    // freely-cloneable indirection that looks up whatever `render_cell`
    // currently holds each time it's called.
    let render_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let trigger_render: Rc<dyn Fn()> = {
        let render_cell = render_cell.clone();
        Rc::new(move || {
            if let Some(f) = render_cell.borrow().as_ref() {
                f();
            }
        })
    };
    // The manual per-thread hide override lives in SQLite, not config, so
    // (unlike the word-list actions, which just need a re-render since
    // render_impl re-reads config live) toggling it needs a DB reload before
    // re-rendering, or the change wouldn't show up until the next catalogue
    // fetch.
    let reload_and_render: Rc<dyn Fn()> = {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let summaries = summaries.clone();
        let trigger_render = trigger_render.clone();
        Rc::new(move || {
            let board = current_board.borrow().clone();
            if let Ok(fresh) = storage::load_cached_threads(&shell.db, &board) {
                *summaries.borrow_mut() = fresh;
            }
            trigger_render();
        })
    };
    // Exposed on Shell so viewer.rs can ask for a reload+re-render after
    // evicting a confirmed-404'd thread from the cache -- same reasoning as
    // Shell::on_boards_changed just above.
    *shell.catalogue_needs_reload.borrow_mut() = Some(reload_and_render.clone());

    {
        let list_box = list_box.clone();
        let list_scroller = list_scroller.clone();
        let summaries = summaries.clone();
        let displayed = displayed.clone();
        let row_popovers = row_popovers.clone();
        let current_sort = current_sort.clone();
        let search_text = search_text.clone();
        let shell = shell.clone();
        let current_board = current_board.clone();
        let last_rendered_board = last_rendered_board.clone();
        let hidden_button = hidden_button.clone();
        let trigger_render = trigger_render.clone();
        let reload_and_render = reload_and_render.clone();
        let render_impl = move || {
            let board_switched = last_rendered_board.borrow().as_deref() != Some(current_board.borrow().as_str());
            *last_rendered_board.borrow_mut() = Some(current_board.borrow().clone());
            // Clearing list_box's children below collapses its content
            // height, which resets the ScrolledWindow's adjustment to 0 --
            // save the pre-clear position here so it can be restored after
            // re-appending, unless this render is an actual board switch
            // (where inheriting the old board's scroll position would be
            // wrong, not helpful).
            let saved_scroll = if board_switched { 0.0 } else { list_scroller.vadjustment().value() };

            let mut items = summaries.borrow().clone();
            sort_threads(&mut items, &current_sort.borrow());
            // Persist the sorted-but-unfiltered list now, before the search
            // query below narrows `items` -- otherwise the first keystroke
            // in the search box would permanently discard every thread it
            // filters out, since `summaries` is meant to be the full list
            // this render (and every future one) starts from.
            *summaries.borrow_mut() = items.clone();

            let query = search_text.borrow().to_lowercase();
            if !query.is_empty() {
                items.retain(|s| {
                    let subject = s.subject.as_deref().unwrap_or("").to_lowercase();
                    subject.contains(&query) || s.comment_preview.to_lowercase().contains(&query)
                });
            }

            let board = current_board.borrow().clone();
            // Global words are unioned in here rather than given their own
            // precedence tier: the existing temp-hide > pin > hide-word >
            // normal order below already does the right thing once a global
            // and a board-specific list are just treated as one combined
            // list -- e.g. a board-specific pin word naturally overrides a
            // global hide word, since any pin match already beats any hide
            // match regardless of which list it came from.
            let (pin_words, hide_words) = {
                let config = shell.config.borrow();
                (
                    config.global_pin_words().iter().chain(config.pin_words_for(&board)).cloned().collect::<Vec<_>>(),
                    config.global_hide_words().iter().chain(config.hide_words_for(&board)).cloned().collect::<Vec<_>>(),
                )
            };

            let mut pinned = Vec::new();
            let mut hidden: Vec<(ThreadSummary, Vec<String>)> = Vec::new();
            let mut visible = Vec::new();
            for s in items.iter() {
                // Temp-hide checked first: overrules even a pin-word match,
                // per the user's "no, I want *this* one hidden regardless"
                // use case.
                if matches!(s.temp_flag, Some(ref f) if f.kind == TempFlagKind::Hide) {
                    let elapsed = now_unix() - s.temp_flag.as_ref().unwrap().flagged_at;
                    let remaining_days = ((storage::TEMP_FLAG_TTL_SECS - elapsed).max(0)) / 86400;
                    let reason = format!("temporarily hidden · expires in {remaining_days}d");
                    hidden.push((s.clone(), vec![reason]));
                    continue;
                }
                let is_temp_pinned = matches!(s.temp_flag, Some(ref f) if f.kind == TempFlagKind::Pin);
                let pin_matches = filters::find_matches(s.subject.as_deref(), &s.comment_preview, &pin_words);
                if is_temp_pinned || !pin_matches.is_empty() {
                    pinned.push(s.clone());
                    continue;
                }
                let hide_matches = filters::find_matches(s.subject.as_deref(), &s.comment_preview, &hide_words);
                if !hide_matches.is_empty() {
                    let reasons = hide_matches.into_iter().map(str::to_string).collect();
                    hidden.push((s.clone(), reasons));
                    continue;
                }
                visible.push(s.clone());
            }
            hidden_button.set_label(&format!("Hidden ({})", hidden.len()));

            for p in row_popovers.borrow_mut().drain(..) {
                p.unparent();
            }
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            let mut shown: Vec<ThreadSummary> = Vec::new();
            if hidden_button.is_active() {
                for (s, reasons) in &hidden {
                    let content = build_row(s, false, Some(reasons));
                    attach_quick_actions(&content, &shell, &current_board, s, &trigger_render, &reload_and_render, &row_popovers);
                    list_box.append(&wrap_in_row(&content, false));
                    shown.push(s.clone());
                }
            } else {
                for s in &pinned {
                    let content = build_row(s, true, None);
                    attach_quick_actions(&content, &shell, &current_board, s, &trigger_render, &reload_and_render, &row_popovers);
                    list_box.append(&wrap_in_row(&content, true));
                    shown.push(s.clone());
                }
                for s in &visible {
                    let content = build_row(s, false, None);
                    attach_quick_actions(&content, &shell, &current_board, s, &trigger_render, &reload_and_render, &row_popovers);
                    list_box.append(&wrap_in_row(&content, false));
                    shown.push(s.clone());
                }
            }
            *displayed.borrow_mut() = shown;

            // Restoring once isn't enough: thumbnails for the newly-rebuilt
            // rows keep arriving asynchronously and resizing rows for a
            // while afterward, which silently undoes an earlier restore --
            // same hazard as the thumbnail-strip scroll in viewer.rs.
            // Retrying over a few staggered delays rides that out; each
            // call is a cheap no-op once nothing's moved.
            if saved_scroll > 0.0 {
                for delay_ms in [0, 150, 500] {
                    let list_scroller = list_scroller.clone();
                    glib::timeout_add_local_once(Duration::from_millis(delay_ms), move || {
                        list_scroller.vadjustment().set_value(saved_scroll);
                    });
                }
            }
        };
        *render_cell.borrow_mut() = Some(Rc::new(render_impl));
    }
    let render = trigger_render.clone();

    let fetch_and_show: Rc<dyn Fn(String, bool)> = {
        let shell = shell.clone();
        let summaries = summaries.clone();
        let render = render.clone();
        let status_label = status_label.clone();
        let last_fetch = last_fetch.clone();
        Rc::new(move |board: String, force: bool| {
            if let Ok(cached) = storage::load_cached_threads(&shell.db, &board) {
                *summaries.borrow_mut() = cached;
                render();
            }

            let now = Instant::now();
            let should_fetch = force
                || last_fetch
                    .borrow()
                    .get(&board)
                    .is_none_or(|t| now.duration_since(*t) >= REFRESH_DEBOUNCE);
            if !should_fetch {
                status_label.set_text("Showing cached threads (refreshed recently; try again shortly).");
                return;
            }
            last_fetch.borrow_mut().insert(board.clone(), now);

            status_label.set_text(&format!("Loading /{board}/..."));
            let shell = shell.clone();
            let summaries = summaries.clone();
            let render = render.clone();
            let status_label = status_label.clone();
            let board_for_result = board.clone();
            net::fetch_catalogue(board, move |result| match result {
                Ok(pages) => {
                    let fetched: Vec<ThreadSummary> = pages
                        .iter()
                        .flat_map(|p| p.threads.iter())
                        .map(|t| ThreadSummary::from_catalog(&board_for_result, t))
                        .collect();
                    if let Err(e) = storage::upsert_thread_summaries(&shell.db, &board_for_result, &fetched) {
                        eprintln!("[storage] failed to cache catalogue: {e}");
                    }
                    let live_numbers: Vec<u64> = fetched.iter().map(|s| s.number).collect();
                    if let Err(e) = storage::prune_vanished_threads(&shell.db, &board_for_result, &live_numbers) {
                        eprintln!("[storage] failed to prune vanished threads: {e}");
                    }
                    match storage::load_cached_threads(&shell.db, &board_for_result) {
                        Ok(merged) => {
                            status_label.set_text(&format!("Loaded {} threads.", merged.len()));
                            *summaries.borrow_mut() = merged;
                            render();
                        }
                        Err(e) => status_label.set_text(&format!("Cache read failed: {e}")),
                    }
                }
                Err(e) => {
                    status_label.set_text(&format!("Fetch failed ({e}); showing cached threads if available."));
                }
            });
        })
    };

    // Self-referencing rebuild closure: board tabs need to rebuild
    // themselves (e.g. after add/remove), so the Rc is populated after
    // construction and each handler looks it up through the cell.
    let rebuild_tabs_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    // Every "Remove board" Popover from the current set of tabs, unparented
    // and cleared at the start of the next rebuild -- see the identical
    // reasoning on `row_popovers` above; `rebuild_tabs` re-creates every tab
    // button (and its popover) from scratch on every call, including a
    // plain board-tab click, not just when boards are actually added/removed.
    let tab_popovers: Rc<RefCell<Vec<gtk4::Popover>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let tabs_box = tabs_box.clone();
        let shell = shell.clone();
        let fetch_and_show = fetch_and_show.clone();
        let current_board = current_board.clone();
        let rebuild_tabs_cell_for_children = rebuild_tabs_cell.clone();
        let tab_popovers = tab_popovers.clone();
        let rebuild_tabs = move || {
            for p in tab_popovers.borrow_mut().drain(..) {
                p.unparent();
            }
            while let Some(child) = tabs_box.first_child() {
                tabs_box.remove(&child);
            }
            let boards = shell.config.borrow().boards.clone();
            let can_remove = boards.len() > 1;
            for board in boards {
                let is_active = *current_board.borrow() == board;
                let button = gtk4::Button::with_label(&format!("/{board}/"));
                if is_active {
                    button.add_css_class("suggested-action");
                }
                {
                    let shell = shell.clone();
                    let fetch_and_show = fetch_and_show.clone();
                    let current_board = current_board.clone();
                    let board = board.clone();
                    let rebuild_tabs_cell = rebuild_tabs_cell_for_children.clone();
                    button.connect_clicked(move |_| {
                        *current_board.borrow_mut() = board.clone();
                        shell.config.borrow_mut().active_board = board.clone();
                        shell.config.borrow().save();
                        storage::set_app_state(&shell.db, "active_board", &board).ok();
                        fetch_and_show(board.clone(), false);
                        if let Some(f) = rebuild_tabs_cell.borrow().as_ref() {
                            f();
                        }
                    });
                }

                // Right-click "Remove board" instead of a permanently visible
                // "×", to keep the tab bar uncluttered. Only offered when
                // more than one board is configured (there must always be
                // somewhere to fall back to).
                if can_remove {
                    let popover = gtk4::Popover::new();
                    popover.set_parent(&button);
                    popover.set_has_arrow(true);
                    tab_popovers.borrow_mut().push(popover.clone());
                    let remove_item_button = gtk4::Button::with_label("Remove board");
                    popover.set_child(Some(&remove_item_button));
                    {
                        let shell = shell.clone();
                        let fetch_and_show = fetch_and_show.clone();
                        let current_board = current_board.clone();
                        let board = board.clone();
                        let rebuild_tabs_cell = rebuild_tabs_cell_for_children.clone();
                        let popover = popover.clone();
                        remove_item_button.connect_clicked(move |_| {
                            shell.config.borrow_mut().boards.retain(|b| b != &board);
                            shell.config.borrow().save();
                            if *current_board.borrow() == board {
                                let next = shell.config.borrow().boards.first().cloned().unwrap_or_else(|| "gif".to_string());
                                *current_board.borrow_mut() = next.clone();
                                shell.config.borrow_mut().active_board = next.clone();
                                shell.config.borrow().save();
                                storage::set_app_state(&shell.db, "active_board", &next).ok();
                                fetch_and_show(next, false);
                            }
                            popover.popdown();
                            if let Some(f) = rebuild_tabs_cell.borrow().as_ref() {
                                f();
                            }
                        });
                    }
                    let gesture = gtk4::GestureClick::new();
                    gesture.set_button(3);
                    {
                        let popover = popover.clone();
                        gesture.connect_pressed(move |_, _, _, _| popover.popup());
                    }
                    button.add_controller(gesture);
                }

                tabs_box.append(&button);
            }
        };
        *rebuild_tabs_cell.borrow_mut() = Some(Rc::new(rebuild_tabs));
    }
    if let Some(f) = rebuild_tabs_cell.borrow().as_ref() {
        f();
    }

    // Lets settings.rs's board list (add/remove) notify this page without
    // reaching into its closures directly -- see Shell::on_boards_changed.
    *shell.on_boards_changed.borrow_mut() = Some({
        let shell = shell.clone();
        let current_board = current_board.clone();
        let fetch_and_show = fetch_and_show.clone();
        let rebuild_tabs_cell = rebuild_tabs_cell.clone();
        Rc::new(move || {
            let boards = shell.config.borrow().boards.clone();
            if !boards.contains(&current_board.borrow().clone()) {
                let next = boards.first().cloned().unwrap_or_else(|| "gif".to_string());
                *current_board.borrow_mut() = next.clone();
                shell.config.borrow_mut().active_board = next.clone();
                shell.config.borrow().save();
                storage::set_app_state(&shell.db, "active_board", &next).ok();
                fetch_and_show(next, false);
            }
            if let Some(f) = rebuild_tabs_cell.borrow().as_ref() {
                f();
            }
        })
    });

    search_entry.connect_changed({
        let search_text = search_text.clone();
        let render = render.clone();
        move |entry| {
            *search_text.borrow_mut() = entry.text().to_string();
            render();
        }
    });

    sort_dropdown.connect_selected_notify({
        let shell = shell.clone();
        let current_sort = current_sort.clone();
        let render = render.clone();
        move |dd| {
            let idx = dd.selected() as usize;
            if let Some(key) = SORT_KEYS.get(idx) {
                *current_sort.borrow_mut() = key.to_string();
                shell.config.borrow_mut().sort_threads_by = key.to_string();
                shell.config.borrow().save();
                render();
            }
        }
    });

    filters_button.connect_clicked({
        let shell = shell.clone();
        let current_board = current_board.clone();
        let render = render.clone();
        move |_| open_filters_editor(&shell, &current_board, &render)
    });

    temporary_button.connect_clicked({
        let shell = shell.clone();
        let current_board = current_board.clone();
        let reload_and_render = reload_and_render.clone();
        move |_| open_temp_flags_panel(&shell, &current_board, &reload_and_render)
    });

    hidden_button.connect_toggled({
        let render = render.clone();
        move |_| render()
    });

    refresh_button.connect_clicked({
        let fetch_and_show = fetch_and_show.clone();
        let current_board = current_board.clone();
        move |_| fetch_and_show(current_board.borrow().clone(), true)
    });

    list_box.connect_row_activated({
        let shell = shell.clone();
        let displayed = displayed.clone();
        move |_, row| {
            let idx = row.index() as usize;
            if let Some(s) = displayed.borrow().get(idx).cloned() {
                viewer::open_thread(&shell, &s.board, s.number);
            }
        }
    });

    fetch_and_show(initial_board, false);
}
