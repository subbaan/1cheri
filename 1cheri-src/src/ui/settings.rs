// Settings window (project.md 0.4.0 milestone): boards, save location, and
// playback defaults, all previously only editable by hand in config.toml or
// (for boards) via the catalogue toolbar's now-removed add-board entry.

use super::{build_editable_list_widget, remember_window_size, Shell};
use crate::{comments, net};
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn open_settings(shell: &Rc<Shell>) {
    let window = gtk4::Window::builder()
        .title("Settings")
        .transient_for(&shell.window)
        .build();
    remember_window_size(
        &window,
        shell,
        |c| (c.settings_window_width, c.settings_window_height),
        |c, w, h| {
            c.settings_window_width = w;
            c.settings_window_height = h;
        },
    );

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 14);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    // Boards
    let on_boards_changed: Rc<dyn Fn()> = {
        let shell = shell.clone();
        Rc::new(move || {
            if let Some(f) = shell.on_boards_changed.borrow().as_ref() {
                f();
            }
        })
    };
    let help_button = gtk4::Button::with_label("?");
    help_button.set_tooltip_text(Some("Browse available boards"));
    help_button.connect_clicked({
        let shell = shell.clone();
        let on_boards_changed = on_boards_changed.clone();
        move |_| open_board_directory(&shell, on_boards_changed.clone())
    });

    let boards_widget = {
        let shell_get = shell.clone();
        let shell_add = shell.clone();
        let shell_remove = shell.clone();
        build_editable_list_widget(
            "Boards",
            "board name (e.g. gif)",
            false,
            move || shell_get.config.borrow().boards.clone(),
            move |board| {
                let mut cfg = shell_add.config.borrow_mut();
                if !cfg.boards.contains(&board) {
                    cfg.boards.push(board);
                }
                drop(cfg);
                shell_add.config.borrow().save();
            },
            move |board| {
                shell_remove.config.borrow_mut().boards.retain(|b| b != board);
                shell_remove.config.borrow().save();
            },
            on_boards_changed,
            Some(help_button.upcast()),
        )
    };
    root.append(&boards_widget);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // Save location
    let save_label = gtk4::Label::new(None);
    save_label.set_markup("<b>Save location</b>");
    save_label.set_xalign(0.0);
    root.append(&save_label);

    let save_dir_entry = gtk4::Entry::new();
    save_dir_entry.set_text(&shell.config.borrow().save_directory);
    save_dir_entry.set_placeholder_text(Some("~/Downloads/1cheri"));
    save_dir_entry.connect_activate({
        let shell = shell.clone();
        move |entry| {
            shell.config.borrow_mut().save_directory = entry.text().to_string();
            shell.config.borrow().save();
        }
    });
    root.append(&gtk4::Label::new(Some("Directory (Enter to apply):")));
    root.append(&save_dir_entry);

    let organize_check = gtk4::CheckButton::with_label("Organize saves into per-thread folders");
    organize_check.set_active(shell.config.borrow().save_organize_by_thread);
    organize_check.set_tooltip_text(Some(
        "On: saves go into <save dir>/<board>/<thread>/. Off: every file is dumped directly into the save directory.",
    ));
    organize_check.connect_toggled({
        let shell = shell.clone();
        move |btn| {
            shell.config.borrow_mut().save_organize_by_thread = btn.is_active();
            shell.config.borrow().save();
        }
    });
    root.append(&organize_check);

    let template_entry = gtk4::Entry::new();
    template_entry.set_text(&shell.config.borrow().filename_template);
    template_entry.set_placeholder_text(Some("{post}_{original_name}"));
    template_entry.connect_activate({
        let shell = shell.clone();
        move |entry| {
            shell.config.borrow_mut().filename_template = entry.text().to_string();
            shell.config.borrow().save();
        }
    });
    root.append(&gtk4::Label::new(Some("Filename template (Enter to apply):")));
    root.append(&template_entry);

    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // Playback defaults
    let playback_label = gtk4::Label::new(None);
    playback_label.set_markup("<b>Playback defaults</b>");
    playback_label.set_xalign(0.0);
    root.append(&playback_label);

    let muted_check = gtk4::CheckButton::with_label("Muted");
    muted_check.set_active(shell.config.borrow().muted);
    muted_check.connect_toggled({
        let shell = shell.clone();
        move |btn| {
            shell.config.borrow_mut().muted = btn.is_active();
            shell.config.borrow().save();
        }
    });
    root.append(&muted_check);

    let loop_check = gtk4::CheckButton::with_label("Loop video");
    loop_check.set_active(shell.config.borrow().loop_video);
    loop_check.connect_toggled({
        let shell = shell.clone();
        move |btn| {
            shell.config.borrow_mut().loop_video = btn.is_active();
            shell.config.borrow().save();
        }
    });
    root.append(&loop_check);

    let autoplay_check = gtk4::CheckButton::with_label("Autoplay");
    autoplay_check.set_active(shell.config.borrow().autoplay);
    autoplay_check.connect_toggled({
        let shell = shell.clone();
        move |btn| {
            shell.config.borrow_mut().autoplay = btn.is_active();
            shell.config.borrow().save();
        }
    });
    root.append(&autoplay_check);

    let resume_check = gtk4::CheckButton::with_label("Resume last thread on startup");
    resume_check.set_active(shell.config.borrow().resume_last_thread);
    resume_check.connect_toggled({
        let shell = shell.clone();
        move |btn| {
            shell.config.borrow_mut().resume_last_thread = btn.is_active();
            shell.config.borrow().save();
        }
    });
    root.append(&resume_check);

    let volume_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    volume_row.append(&gtk4::Label::new(Some("Volume:")));
    let volume_spin = gtk4::SpinButton::with_range(0.0, 100.0, 5.0);
    volume_spin.set_value(shell.config.borrow().volume as f64);
    volume_spin.connect_value_changed({
        let shell = shell.clone();
        move |spin| {
            shell.config.borrow_mut().volume = spin.value() as i64;
            shell.config.borrow().save();
        }
    });
    volume_row.append(&volume_spin);
    root.append(&volume_row);

    root.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    // Diagnostics
    let diagnostics_label = gtk4::Label::new(None);
    diagnostics_label.set_markup("<b>Diagnostics</b>");
    diagnostics_label.set_xalign(0.0);
    root.append(&diagnostics_label);

    let log_button = gtk4::Button::with_label("Open log file");
    log_button.set_tooltip_text(Some(&crate::logging::log_path().display().to_string()));
    log_button.connect_clicked(|_| {
        let uri = format!("file://{}", crate::logging::log_path().display());
        let ctx: Option<&gtk4::gio::AppLaunchContext> = None;
        if let Err(e) = gtk4::gio::AppInfo::launch_default_for_uri(&uri, ctx) {
            eprintln!("[settings] failed to open {uri}: {e}");
        }
    });
    root.append(&log_button);

    window.set_child(Some(&root));
    window.present();
}

/// Board directory ("?" next to Add in the Boards section): fetches the
/// real board list from 4chan (`net::fetch_boards`) so a user can actually
/// see what boards exist and what they're for, rather than needing to
/// already know a board code to type into the Add box. Clicking an entry
/// adds it directly (same "push if not already present, then save" logic
/// already used by the Boards list's own Add button) and closes the window.
fn open_board_directory(shell: &Rc<Shell>, on_boards_changed: Rc<dyn Fn()>) {
    let window = gtk4::Window::builder().title("Board directory").transient_for(&shell.window).build();
    remember_window_size(
        &window,
        shell,
        |c| (c.board_directory_window_width, c.board_directory_window_height),
        |c, w, h| {
            c.board_directory_window_width = w;
            c.board_directory_window_height = h;
        },
    );

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);

    let search_entry = gtk4::Entry::new();
    search_entry.set_placeholder_text(Some("Search boards..."));
    root.append(&search_entry);

    let status_label = gtk4::Label::new(Some("Loading board list..."));
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");
    root.append(&status_label);

    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::None);
    list_box.set_activate_on_single_click(true);
    let list_scroller = gtk4::ScrolledWindow::new();
    list_scroller.set_child(Some(&list_box));
    list_scroller.set_vexpand(true);
    root.append(&list_scroller);

    window.set_child(Some(&root));
    window.present();

    let boards: Rc<RefCell<Vec<crate::models::BoardInfo>>> = Rc::new(RefCell::new(Vec::new()));
    // Board codes in exactly the order they're currently appended to
    // `list_box`, so `connect_row_activated`'s row index can be mapped back
    // to which board was clicked (the list is filtered/rebuilt as the user
    // types, so this can't just be `boards` itself).
    let shown_codes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let rebuild_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    {
        let list_box = list_box.clone();
        let boards = boards.clone();
        let shown_codes = shown_codes.clone();
        let search_entry = search_entry.clone();
        let rebuild = move || {
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            let query = search_entry.text().to_lowercase();
            let mut codes = Vec::new();
            for b in boards.borrow().iter() {
                if !query.is_empty()
                    && !b.board.to_lowercase().contains(&query)
                    && !b.title.to_lowercase().contains(&query)
                    && !b.meta_description.to_lowercase().contains(&query)
                {
                    continue;
                }
                let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                row.set_margin_start(6);
                row.set_margin_end(6);

                let nsfw_suffix = if b.ws_board == 0 { "  [18+]" } else { "" };
                let title_label = gtk4::Label::new(None);
                title_label.set_markup(&format!(
                    "<b>/{}/ \u{2014} {}</b>{}",
                    glib::markup_escape_text(&b.board),
                    glib::markup_escape_text(&b.title),
                    nsfw_suffix,
                ));
                title_label.set_xalign(0.0);
                row.append(&title_label);

                let description = comments::unescape_entities(&b.meta_description);
                if !description.is_empty() {
                    let desc_label = gtk4::Label::new(Some(&description));
                    desc_label.set_xalign(0.0);
                    desc_label.set_wrap(true);
                    desc_label.add_css_class("dim-label");
                    row.append(&desc_label);
                }

                list_box.append(&row);
                codes.push(b.board.clone());
            }
            *shown_codes.borrow_mut() = codes;
        };
        *rebuild_cell.borrow_mut() = Some(Rc::new(rebuild));
    }
    if let Some(f) = rebuild_cell.borrow().as_ref() {
        f();
    }

    search_entry.connect_changed({
        let rebuild_cell = rebuild_cell.clone();
        move |_| {
            if let Some(f) = rebuild_cell.borrow().as_ref() {
                f();
            }
        }
    });

    list_box.connect_row_activated({
        let shown_codes = shown_codes.clone();
        let shell = shell.clone();
        let window = window.clone();
        move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            let Some(code) = shown_codes.borrow().get(index as usize).cloned() else {
                return;
            };
            let mut cfg = shell.config.borrow_mut();
            if !cfg.boards.contains(&code) {
                cfg.boards.push(code);
            }
            drop(cfg);
            shell.config.borrow().save();
            on_boards_changed();
            window.close();
        }
    });

    net::fetch_boards({
        let boards = boards.clone();
        let status_label = status_label.clone();
        let rebuild_cell = rebuild_cell.clone();
        move |result| match result {
            Ok(list) => {
                status_label.set_text(&format!("{} boards", list.len()));
                *boards.borrow_mut() = list;
                if let Some(f) = rebuild_cell.borrow().as_ref() {
                    f();
                }
            }
            Err(e) => {
                status_label.set_text(&format!("Failed to load board list: {e}"));
            }
        }
    });
}
