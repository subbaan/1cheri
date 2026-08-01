// Settings window (project.md 0.4.0 milestone): boards, save location, and
// playback defaults, all previously only editable by hand in config.toml or
// (for boards) via the catalogue toolbar's now-removed add-board entry.

use super::{build_editable_list_widget, Shell};
use gtk4::prelude::*;
use std::rc::Rc;

pub fn open_settings(shell: &Rc<Shell>) {
    let window = gtk4::Window::builder()
        .title("Settings")
        .transient_for(&shell.window)
        .default_width(360)
        .default_height(520)
        .build();

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

    window.set_child(Some(&root));
    window.present();
}
