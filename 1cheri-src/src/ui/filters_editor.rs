//! The Filters window: a sidebar of the four pin/hide word lists (the
//! selected board's own, and the global ones) with only the selected list
//! shown on the right, as removable chips with live match counts and a
//! right-click menu to edit a word or move it to another list.

use super::Shell;
use crate::config::{FilterKind, FilterScope};
use crate::filters;
use crate::models::ThreadSummary;
use crate::storage;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const SORT_LABELS: [&str; 3] = ["Order added", "A → Z", "Most matches"];
const SORT_KEYS: [&str; 3] = ["added", "alpha", "matches"];

/// Sidebar row index -> (global?, kind). Rows 0 and 3 are the non-selectable
/// "BOARD" / "ALL BOARDS" headers.
fn section_for_row(index: i32) -> Option<(bool, FilterKind)> {
    match index {
        1 => Some((false, FilterKind::Pin)),
        2 => Some((false, FilterKind::Hide)),
        4 => Some((true, FilterKind::Pin)),
        5 => Some((true, FilterKind::Hide)),
        _ => None,
    }
}

fn kind_name(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Pin => "Pin",
        FilterKind::Hide => "Hide",
    }
}

fn other_kind(kind: FilterKind) -> FilterKind {
    match kind {
        FilterKind::Pin => FilterKind::Hide,
        FilterKind::Hide => FilterKind::Pin,
    }
}

/// Boards offered in the sidebar's board picker: the configured tabs first,
/// then any other board that still has filter words saved for it (e.g. one
/// since removed from Settings), so those words stay reachable.
fn board_choices(shell: &Shell, current: &str) -> Vec<String> {
    let config = shell.config.borrow();
    let mut boards: Vec<String> = config.boards.clone();
    if !boards.iter().any(|b| b == current) {
        boards.insert(0, current.to_string());
    }
    for f in &config.board_filters {
        if (!f.pin_words.is_empty() || !f.hide_words.is_empty()) && !boards.contains(&f.board) {
            boards.push(f.board.clone());
        }
    }
    boards
}

/// How many threads each of `words` matches in `threads`, using the same
/// matcher as the catalogue. Raw per-word matches, ignoring precedence (a
/// hide word still counts a thread that a pin word would keep visible).
fn match_counts(threads: &[ThreadSummary], words: &[String]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = words.iter().map(|w| (w.clone(), 0)).collect();
    for t in threads {
        for m in filters::find_matches(t.subject.as_deref(), &t.comment_preview, words) {
            if let Some(c) = counts.get_mut(m) {
                *c += 1;
            }
        }
    }
    counts
}

fn sidebar_row(text: &str, count_label: &gtk4::Label) -> gtk4::ListBoxRow {
    let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let label = gtk4::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    count_label.add_css_class("dim-label");
    row_box.append(&label);
    row_box.append(count_label);
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}

fn sidebar_header(text: &str, extra: Option<&gtk4::Widget>) -> gtk4::ListBoxRow {
    let header_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let label = gtk4::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("filters-sidebar-header");
    header_box.append(&label);
    if let Some(widget) = extra {
        header_box.append(widget);
    }
    let row = gtk4::ListBoxRow::new();
    row.set_child(Some(&header_box));
    row.set_selectable(false);
    row.set_activatable(false);
    row
}

pub fn open(
    shell: &Rc<Shell>,
    current_board: &Rc<RefCell<String>>,
    summaries: &Rc<RefCell<Vec<ThreadSummary>>>,
    trigger_render: &Rc<dyn Fn()>,
) {
    let window = gtk4::Window::builder().title("Filters").transient_for(&shell.window).build();
    super::remember_window_size(
        &window,
        shell,
        |c| (c.filters_window_width, c.filters_window_height),
        |c, w, h| {
            c.filters_window_width = w;
            c.filters_window_height = h;
        },
    );
    // Sizes remembered from the old single-column layout are too narrow
    // for sidebar + chips side by side.
    if window.default_width() < 560 {
        window.set_default_size(560, window.default_height());
    }

    let boards = board_choices(shell, &current_board.borrow());
    let board: Rc<RefCell<String>> = Rc::new(RefCell::new(current_board.borrow().clone()));
    let section: Rc<Cell<(bool, FilterKind)>> = Rc::new(Cell::new((false, FilterKind::Hide)));

    // --- Sidebar ---
    let board_labels: Vec<String> = boards.iter().map(|b| format!("/{b}/")).collect();
    let board_label_refs: Vec<&str> = board_labels.iter().map(String::as_str).collect();
    let board_dropdown = gtk4::DropDown::from_strings(&board_label_refs);
    board_dropdown.set_selected(boards.iter().position(|b| *b == *board.borrow()).unwrap_or(0) as u32);

    let board_pin_count = gtk4::Label::new(None);
    let board_hide_count = gtk4::Label::new(None);
    let global_pin_count = gtk4::Label::new(None);
    let global_hide_count = gtk4::Label::new(None);

    let sidebar = gtk4::ListBox::new();
    sidebar.add_css_class("filters-sidebar");
    sidebar.set_selection_mode(gtk4::SelectionMode::Single);
    sidebar.append(&sidebar_header("BOARD", Some(board_dropdown.upcast_ref())));
    sidebar.append(&sidebar_row("Pin words", &board_pin_count));
    sidebar.append(&sidebar_row("Hide words", &board_hide_count));
    sidebar.append(&sidebar_header("ALL BOARDS", None));
    sidebar.append(&sidebar_row("Pin words", &global_pin_count));
    sidebar.append(&sidebar_row("Hide words", &global_hide_count));
    sidebar.set_size_request(180, -1);

    // --- Right pane ---
    let pane = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    pane.set_margin_top(12);
    pane.set_margin_bottom(10);
    pane.set_margin_start(14);
    pane.set_margin_end(14);
    pane.set_hexpand(true);

    let title_label = gtk4::Label::new(None);
    title_label.set_xalign(0.0);
    let subtitle_label = gtk4::Label::new(None);
    subtitle_label.set_xalign(0.0);
    subtitle_label.set_wrap(true);
    subtitle_label.add_css_class("dim-label");

    let entry_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    let entry = gtk4::Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Add a word or phrase (separate several with , or |)"));
    let add_button = gtk4::Button::with_label("Add");
    entry_row.append(&entry);
    entry_row.append(&add_button);

    let flow_box = gtk4::FlowBox::new();
    flow_box.set_selection_mode(gtk4::SelectionMode::None);
    flow_box.set_max_children_per_line(u32::MAX);
    flow_box.set_homogeneous(false);
    flow_box.set_row_spacing(4);
    flow_box.set_column_spacing(4);
    flow_box.set_valign(gtk4::Align::Start);
    let empty_label = gtk4::Label::new(None);
    empty_label.add_css_class("dim-label");
    empty_label.set_margin_top(24);
    let chips_area = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    chips_area.append(&flow_box);
    chips_area.append(&empty_label);
    let chips_scroller = gtk4::ScrolledWindow::new();
    chips_scroller.set_child(Some(&chips_area));
    chips_scroller.set_vexpand(true);
    chips_scroller.set_hscrollbar_policy(gtk4::PolicyType::Never);

    let footer = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let footer_label = gtk4::Label::new(None);
    footer_label.set_xalign(0.0);
    footer_label.set_hexpand(true);
    footer_label.add_css_class("dim-label");
    let sort_dropdown = gtk4::DropDown::from_strings(&SORT_LABELS);
    let initial_sort = SORT_KEYS.iter().position(|k| *k == shell.config.borrow().filters_sort).unwrap_or(0);
    sort_dropdown.set_selected(initial_sort as u32);
    footer.append(&footer_label);
    footer.append(&sort_dropdown);

    pane.append(&title_label);
    pane.append(&subtitle_label);
    pane.append(&entry_row);
    pane.append(&chips_scroller);
    pane.append(&footer);

    let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    root.append(&sidebar);
    root.append(&gtk4::Separator::new(gtk4::Orientation::Vertical));
    root.append(&pane);

    let scope_for = {
        let board = board.clone();
        move |global: bool| if global { FilterScope::Global } else { FilterScope::Board(board.borrow().clone()) }
    };

    // Same deal as catalogue.rs's row popovers: `set_parent` doesn't give
    // the chip an owning reference, so these must be unparented before the
    // chips are discarded on each rebuild.
    let chip_popovers: Rc<RefCell<Vec<gtk4::Popover>>> = Rc::new(RefCell::new(Vec::new()));
    let refresh_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    // Every edit saves, re-renders the catalogue behind the window, and
    // rebuilds this window's counts and chips.
    let after_change: Rc<dyn Fn()> = {
        let shell = shell.clone();
        let trigger_render = trigger_render.clone();
        let refresh_cell = refresh_cell.clone();
        Rc::new(move || {
            shell.config.borrow().save();
            trigger_render();
            if let Some(f) = refresh_cell.borrow().clone() {
                f();
            }
        })
    };

    {
        let shell = shell.clone();
        let current_board = current_board.clone();
        let summaries = summaries.clone();
        let board = board.clone();
        let section = section.clone();
        let scope_for = scope_for.clone();
        let flow_box = flow_box.clone();
        let empty_label = empty_label.clone();
        let chip_popovers = chip_popovers.clone();
        let after_change = after_change.clone();
        let sort_dropdown = sort_dropdown.clone();
        let refresh = move || {
            let board_name = board.borrow().clone();
            let (global, kind) = section.get();
            let scope = scope_for(global);
            let board_scope = FilterScope::Board(board_name.clone());

            {
                let config = shell.config.borrow();
                board_pin_count.set_text(&config.filter_words(&board_scope, FilterKind::Pin).len().to_string());
                board_hide_count.set_text(&config.filter_words(&board_scope, FilterKind::Hide).len().to_string());
                global_pin_count.set_text(&config.global_pin_words().len().to_string());
                global_hide_count.set_text(&config.global_hide_words().len().to_string());
            }

            let noun = if kind == FilterKind::Pin { "Pin words" } else { "Hide words" };
            let (title, subtitle) = if global {
                (
                    format!("{noun} — all boards"),
                    if kind == FilterKind::Pin {
                        "Applies to every board. Matching threads are highlighted and sorted to the top.".to_string()
                    } else {
                        "Applies to every board. Matching threads are moved to Hidden.".to_string()
                    },
                )
            } else {
                (
                    format!("{noun} — /{board_name}/"),
                    if kind == FilterKind::Pin {
                        format!("Threads on /{board_name}/ matching any of these are highlighted and sorted to the top.")
                    } else {
                        format!("Threads on /{board_name}/ matching any of these are moved to Hidden.")
                    },
                )
            };
            title_label.set_markup(&format!("<span size='large'><b>{}</b></span>", glib::markup_escape_text(&title)));
            subtitle_label.set_text(&subtitle);

            let mut words = shell.config.borrow().filter_words(&scope, kind).to_vec();
            // Counts are against the picked board's catalogue: the live one
            // if it's the board the catalogue is showing, otherwise whatever
            // was cached from its last visit (none if it was never opened).
            let threads = if *current_board.borrow() == board_name {
                summaries.borrow().clone()
            } else {
                storage::load_cached_threads(&shell.db, &board_name).unwrap_or_default()
            };
            let counts = if threads.is_empty() { None } else { Some(match_counts(&threads, &words)) };
            let count_of = |w: &str| counts.as_ref().and_then(|c| c.get(w).copied());

            match SORT_KEYS.get(sort_dropdown.selected() as usize).copied().unwrap_or("added") {
                "alpha" => words.sort_by_key(|w| w.to_lowercase()),
                // Stable sort, so equal counts keep their added order.
                "matches" => words.sort_by(|a, b| count_of(b).cmp(&count_of(a))),
                _ => {}
            }

            for p in chip_popovers.borrow_mut().drain(..) {
                p.unparent();
            }
            while let Some(child) = flow_box.first_child() {
                flow_box.remove(&child);
            }

            for word in &words {
                let count = count_of(word);
                let chip = build_chip(word, count, &board_name);
                attach_chip_menu(
                    &chip,
                    &shell,
                    &scope,
                    kind,
                    word,
                    &board_name,
                    &after_change,
                    &chip_popovers,
                );
                flow_box.append(&chip);
            }

            empty_label.set_visible(words.is_empty());
            empty_label.set_text(&format!("No {} yet.", noun.to_lowercase()));

            let mut footer_text = format!("{} {}", words.len(), if words.len() == 1 { "word" } else { "words" });
            if let Some(counts) = &counts {
                let unused = words.iter().filter(|w| counts.get(w.as_str()) == Some(&0)).count();
                if unused > 0 {
                    footer_text.push_str(&format!(" · {unused} match nothing on /{board_name}/ right now"));
                }
            } else {
                footer_text.push_str(&format!(" · no cached /{board_name}/ catalogue to count matches against"));
            }
            footer_label.set_text(&footer_text);
        };
        *refresh_cell.borrow_mut() = Some(Rc::new(refresh));
    }
    let refresh: Rc<dyn Fn()> = refresh_cell.borrow().clone().unwrap();

    // Adding: "," and "|" split into several entries at once, as before.
    let add_from_entry: Rc<dyn Fn()> = {
        let shell = shell.clone();
        let entry = entry.clone();
        let section = section.clone();
        let scope_for = scope_for.clone();
        let after_change = after_change.clone();
        Rc::new(move || {
            let text = entry.text().to_string();
            if text.trim().is_empty() {
                return;
            }
            let (global, kind) = section.get();
            let scope = scope_for(global);
            {
                let mut config = shell.config.borrow_mut();
                for piece in text.split(['|', ',']) {
                    config.add_filter_word(&scope, kind, piece.to_string());
                }
            }
            entry.set_text("");
            after_change();
        })
    };
    {
        let add_from_entry = add_from_entry.clone();
        add_button.connect_clicked(move |_| add_from_entry());
    }
    entry.connect_activate(move |_| add_from_entry());

    sidebar.connect_row_selected({
        let section = section.clone();
        let refresh = refresh.clone();
        let entry = entry.clone();
        move |_, row| {
            if let Some(s) = row.and_then(|r| section_for_row(r.index())) {
                section.set(s);
                refresh();
                entry.grab_focus();
            }
        }
    });
    board_dropdown.connect_selected_notify({
        let board = board.clone();
        let refresh = refresh.clone();
        move |dd| {
            if let Some(b) = boards.get(dd.selected() as usize) {
                *board.borrow_mut() = b.clone();
                refresh();
            }
        }
    });
    sort_dropdown.connect_selected_notify({
        let shell = shell.clone();
        let refresh = refresh.clone();
        move |dd| {
            if let Some(key) = SORT_KEYS.get(dd.selected() as usize) {
                shell.config.borrow_mut().filters_sort = key.to_string();
                shell.config.borrow().save();
                refresh();
            }
        }
    });

    // Start on this board's hide words, the list that gets used most.
    sidebar.select_row(sidebar.row_at_index(2).as_ref());

    window.set_child(Some(&root));
    window.present();
    entry.grab_focus();
}

fn build_chip(word: &str, count: Option<usize>, board: &str) -> gtk4::Box {
    let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    chip.add_css_class("filter-chip");
    chip.set_halign(gtk4::Align::Start);
    chip.set_margin_top(2);
    chip.set_margin_bottom(2);
    let label = gtk4::Label::new(Some(word));
    chip.append(&label);
    if let Some(count) = count {
        let count_label = gtk4::Label::new(Some(&count.to_string()));
        count_label.add_css_class("filter-chip-count");
        chip.append(&count_label);
        if count == 0 {
            chip.add_css_class("filter-chip-unused");
        }
        let tooltip = match count {
            0 => format!("Matches no threads on /{board}/ right now"),
            1 => format!("Matches 1 thread on /{board}/ right now"),
            n => format!("Matches {n} threads on /{board}/ right now"),
        };
        chip.set_tooltip_text(Some(&format!("{tooltip} — right-click to edit or move")));
    } else {
        chip.set_tooltip_text(Some("Right-click to edit or move"));
    }
    chip
}

/// Right-click menu on a chip: edit the word in place, flip it between pin
/// and hide, move it between this board and all boards, or remove it. The
/// remove "×" stays on the chip itself too, for the common case.
#[allow(clippy::too_many_arguments)]
fn attach_chip_menu(
    chip: &gtk4::Box,
    shell: &Rc<Shell>,
    scope: &FilterScope,
    kind: FilterKind,
    word: &str,
    board: &str,
    after_change: &Rc<dyn Fn()>,
    chip_popovers: &Rc<RefCell<Vec<gtk4::Popover>>>,
) {
    let remove_button = gtk4::Button::with_label("×");
    remove_button.add_css_class("flat");
    remove_button.set_tooltip_text(Some("Remove"));
    chip.append(&remove_button);
    {
        let shell = shell.clone();
        let scope = scope.clone();
        let word = word.to_string();
        let after_change = after_change.clone();
        remove_button.connect_clicked(move |_| {
            shell.config.borrow_mut().remove_filter_word(&scope, kind, &word);
            after_change();
        });
    }

    let popover = gtk4::Popover::new();
    popover.set_parent(chip);
    popover.set_has_arrow(true);
    chip_popovers.borrow_mut().push(popover.clone());

    let menu_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    let edit_button = gtk4::Button::with_label("Edit…");
    let flip_kind = other_kind(kind);
    let flip_button = gtk4::Button::with_label(&format!("Move to {} words", kind_name(flip_kind).to_lowercase()));
    let other_scope = match scope {
        FilterScope::Global => FilterScope::Board(board.to_string()),
        FilterScope::Board(_) => FilterScope::Global,
    };
    let scope_button = gtk4::Button::with_label(&match other_scope {
        FilterScope::Global => "Move to all boards".to_string(),
        FilterScope::Board(ref b) => format!("Move to /{b}/ only"),
    });
    let remove_menu_button = gtk4::Button::with_label("Remove");
    menu_box.append(&edit_button);
    menu_box.append(&flip_button);
    menu_box.append(&scope_button);
    menu_box.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    menu_box.append(&remove_menu_button);

    let edit_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    let edit_entry = gtk4::Entry::new();
    edit_entry.set_width_chars(28);
    let edit_buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    edit_buttons.set_halign(gtk4::Align::End);
    let cancel_button = gtk4::Button::with_label("Cancel");
    let save_button = gtk4::Button::with_label("Save");
    save_button.add_css_class("suggested-action");
    edit_buttons.append(&cancel_button);
    edit_buttons.append(&save_button);
    edit_box.append(&edit_entry);
    edit_box.append(&edit_buttons);

    let popover_stack = gtk4::Stack::new();
    popover_stack.set_transition_type(gtk4::StackTransitionType::None);
    popover_stack.set_vhomogeneous(false);
    popover_stack.set_hhomogeneous(false);
    popover_stack.add_named(&menu_box, Some("menu"));
    popover_stack.add_named(&edit_box, Some("edit"));
    popover.set_child(Some(&popover_stack));

    // Each action pops the menu down before changing anything, since the
    // change rebuilds the chips and unparents this popover.
    let run = {
        let popover = popover.clone();
        let after_change = after_change.clone();
        move |action: Box<dyn Fn()>| {
            popover.popdown();
            action();
            after_change();
        }
    };

    {
        let popover_stack = popover_stack.clone();
        let edit_entry = edit_entry.clone();
        let word = word.to_string();
        edit_button.connect_clicked(move |_| {
            edit_entry.set_text(&word);
            popover_stack.set_visible_child_name("edit");
            edit_entry.grab_focus();
            edit_entry.select_region(0, -1);
        });
    }
    {
        let run = run.clone();
        let shell = shell.clone();
        let scope = scope.clone();
        let word = word.to_string();
        flip_button.connect_clicked(move |_| {
            let (shell, scope, word) = (shell.clone(), scope.clone(), word.clone());
            run(Box::new(move || {
                shell.config.borrow_mut().move_filter_word((&scope, kind), (&scope, flip_kind), &word)
            }));
        });
    }
    {
        let run = run.clone();
        let shell = shell.clone();
        let scope = scope.clone();
        let word = word.to_string();
        scope_button.connect_clicked(move |_| {
            let (shell, scope, other_scope, word) = (shell.clone(), scope.clone(), other_scope.clone(), word.clone());
            run(Box::new(move || {
                shell.config.borrow_mut().move_filter_word((&scope, kind), (&other_scope, kind), &word)
            }));
        });
    }
    {
        let run = run.clone();
        let shell = shell.clone();
        let scope = scope.clone();
        let word = word.to_string();
        remove_menu_button.connect_clicked(move |_| {
            let (shell, scope, word) = (shell.clone(), scope.clone(), word.clone());
            run(Box::new(move || shell.config.borrow_mut().remove_filter_word(&scope, kind, &word)));
        });
    }
    let save: Rc<dyn Fn()> = {
        let run = run.clone();
        let shell = shell.clone();
        let scope = scope.clone();
        let word = word.to_string();
        let edit_entry = edit_entry.clone();
        Rc::new(move || {
            let new_word = edit_entry.text().to_string();
            if new_word.trim().is_empty() {
                return;
            }
            let (shell, scope, word) = (shell.clone(), scope.clone(), word.clone());
            run(Box::new(move || {
                shell.config.borrow_mut().rename_filter_word(&scope, kind, &word, new_word.clone())
            }));
        })
    };
    {
        let save = save.clone();
        save_button.connect_clicked(move |_| save());
    }
    edit_entry.connect_activate(move |_| save());
    {
        let popover = popover.clone();
        cancel_button.connect_clicked(move |_| popover.popdown());
    }
    {
        let popover_stack = popover_stack.clone();
        popover.connect_closed(move |_| popover_stack.set_visible_child_name("menu"));
    }

    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3);
    {
        let popover = popover.clone();
        gesture.connect_pressed(move |_, _, x, y| {
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    chip.add_controller(gesture);
}
