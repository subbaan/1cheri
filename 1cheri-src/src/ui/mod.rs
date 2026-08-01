mod catalogue;
mod settings;
mod viewer;

use crate::config::Config;
use crate::player::PlayerHandle;
use crate::storage;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use rusqlite::Connection;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// Everything the viewer page creates that needs explicit teardown when the
/// user opens a different thread or backs out to the catalogue: without
/// this, each opened thread would leak its GLArea's mpv instance and 16ms
/// render-poll timer forever (they don't get cleaned up just because their
/// widgets are removed from the gtk4::Stack).
pub struct ViewerSession {
    pub key_controller: gtk4::EventControllerKey,
    pub gl_area: gtk4::GLArea,
    pub player: PlayerHandle,
    pub player_poll_source: RefCell<Option<glib::SourceId>>,
    pub scrub_poll_source: RefCell<Option<glib::SourceId>>,
}

/// Shared state every page (catalogue, viewer) needs. One instance per
/// running application, held alive by the closures each page attaches.
pub struct Shell {
    pub window: gtk4::ApplicationWindow,
    pub stack: gtk4::Stack,
    pub config: Rc<RefCell<Config>>,
    pub db: Rc<Connection>,
    /// Local media directory for the bundled offline fixture board (see
    /// main.rs `--fixture`); unused for real network-backed boards.
    pub fixture_media_dir: PathBuf,
    pub viewer_session: RefCell<Option<ViewerSession>>,
    /// Set by the catalogue page once it's built; called by the settings
    /// window after a board is added/removed there, so the catalogue's tab
    /// bar (and active-board selection, if the removed board was active)
    /// stays in sync without settings.rs needing to reach into catalogue.rs's
    /// internal closures directly.
    pub on_boards_changed: RefCell<Option<Rc<dyn Fn()>>>,
    /// Set by the catalogue page once it's built; used by viewer.rs to report
    /// a thread-open failure (404, network error) somewhere visible. Without
    /// this, opening a thread that fails to fetch used to fail completely
    /// silently -- nothing on screen changed, and only stderr recorded it.
    pub catalogue_status: RefCell<Option<gtk4::Label>>,
    /// Set by the catalogue page once it's built; called by viewer.rs after
    /// evicting a confirmed-404'd thread from the cache, so it disappears
    /// from the catalogue's list immediately on returning to it rather than
    /// waiting for the next board refresh's own pruning pass.
    pub catalogue_needs_reload: RefCell<Option<Rc<dyn Fn()>>>,
}

impl Shell {
    /// Tears down the current viewer session, if any: removes the window
    /// key controller, stops both poll timers, and drops the Player (which
    /// cleanly shuts down mpv via its Drop impl). Safe to call when there is
    /// no active session.
    pub fn teardown_viewer_session(&self) {
        if let Some(session) = self.viewer_session.borrow_mut().take() {
            self.window.remove_controller(&session.key_controller);
            if let Some(source) = session.player_poll_source.borrow_mut().take() {
                source.remove();
            }
            if let Some(source) = session.scrub_poll_source.borrow_mut().take() {
                source.remove();
            }
            // mpv's render API requires its GL context current when freeing
            // the render context (mpv_render_context_free); make sure that's
            // still true right before the Player (and its RenderContext) is
            // dropped, rather than assuming whatever context happens to be
            // current at teardown time is the right one.
            session.gl_area.make_current();
            *session.player.borrow_mut() = None;
        }
    }

    /// Leaves the viewer and returns to the catalogue -- shared by the
    /// viewer's B/Escape key binding and its top-bar Back button, so both
    /// stay in sync with each other.
    pub fn return_to_catalogue(&self) {
        self.window.set_title(Some("1cheri"));
        self.teardown_viewer_session();
        self.stack.set_visible_child_name("catalogue");
        // Explicitly leaving a thread means "resume last thread" (if
        // enabled) shouldn't bring it back next launch -- only quitting
        // while still inside a thread should resume it. `active_thread` is
        // read back via `.parse::<u64>()` at startup, so clearing it to an
        // empty string (rather than adding a delete/remove storage helper
        // just for this) is enough to make that parse fail and fall
        // through to "no thread to resume".
        storage::set_app_state(&self.db, "active_thread", "").ok();
    }
}

pub fn build_window(app: &gtk4::Application, config: Config, fixture_media_dir: PathBuf, open_fixture: bool) {
    let db = storage::open().expect("failed to open sqlite database");

    if let Err(e) = storage::prune_expired_temp_flags(&db) {
        eprintln!("[storage] failed to prune expired temp flags: {e}");
    }

    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
    // Belt and suspenders: XFCE's xsettingsd keeps pushing its own theme
    // (e.g. FlatColor, no dark variant) over both GTK_THEME and the
    // prefer-dark-theme setting above, so neither reliably sticks on this
    // desktop. A manually-loaded stylesheet at APPLICATION priority always
    // wins regardless of what the session's theme negotiation decides.
    if let Some(display) = gtk4::gdk::Display::default() {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(DARK_THEME_CSS);
        gtk4::style_context_add_provider_for_display(&display, &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let stack = gtk4::Stack::new();
    // No crossfade: with the default transition, a newly-switched-to page's
    // widgets (notably the viewer's GLArea) aren't realized synchronously,
    // so the initial media load -- which runs right after switching pages --
    // would silently race the player's own realize handler and lose.
    stack.set_transition_type(gtk4::StackTransitionType::None);

    // Floating overlay buttons (Back, Settings, Open save folder) were
    // replaced by page-anchored toolbars (viewer's top bar, both pages'
    // overflow menu -- see build_overflow_menu below): a floating Back
    // button turned out to sit over the thumbnail strip rather than the
    // video, and read as a media control rather than app navigation. No
    // overlay is needed anymore now that nothing floats.
    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("1cheri")
        .default_width(1150)
        .default_height(740)
        .child(&stack)
        .build();

    let shell = Rc::new(Shell {
        window: window.clone(),
        stack: stack.clone(),
        config: Rc::new(RefCell::new(config)),
        db: Rc::new(db),
        fixture_media_dir,
        viewer_session: RefCell::new(None),
        on_boards_changed: RefCell::new(None),
        catalogue_status: RefCell::new(None),
        catalogue_needs_reload: RefCell::new(None),
    });

    // Global quit, independent of which page (catalogue or viewer) is
    // showing -- attached once here rather than per-page, since the viewer's
    // own key controller is session-scoped (added/removed with each opened
    // thread, see ViewerSession) and the catalogue previously had no key
    // controller at all, which is why bare `Q` (viewer.rs, now removed) used
    // to only work while a thread was open. Ctrl+Q instead of bare Q also
    // avoids colliding with the catalogue's search box, where `q` is a
    // perfectly normal character to type.
    let quit_controller = gtk4::EventControllerKey::new();
    {
        let window = shell.window.clone();
        quit_controller.connect_key_pressed(move |_ctrl, key, _code, modifiers| {
            if key == gdk::Key::q && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    shell.window.add_controller(quit_controller);

    // Startup-only pruning (above) wouldn't catch temp flags that expire
    // mid-session on a long-running instance -- sweep hourly too.
    glib::timeout_add_local(std::time::Duration::from_secs(3600), {
        let db = shell.db.clone();
        move || {
            if let Err(e) = storage::prune_expired_temp_flags(&db) {
                eprintln!("[storage] failed to prune expired temp flags: {e}");
            }
            glib::ControlFlow::Continue
        }
    });

    let board = storage::get_app_state(&shell.db, "active_board")
        .ok()
        .flatten()
        .unwrap_or_else(|| shell.config.borrow().active_board.clone());

    catalogue::build_page(&shell, board.clone());
    stack.set_visible_child_name("catalogue");

    if open_fixture {
        viewer::open_fixture_thread(&shell);
    } else if shell.config.borrow().resume_last_thread {
        let last_thread = storage::get_app_state(&shell.db, "active_thread")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(number) = last_thread {
            viewer::open_thread(&shell, &board, number);
        }
    }

    window.present();
}

fn open_save_folder(shell: &Rc<Shell>) {
    let path = shell.config.borrow().save_directory_expanded();
    if let Err(e) = std::fs::create_dir_all(&path) {
        eprintln!("[settings] failed to create save directory {}: {e}", path.display());
        return;
    }
    let uri = format!("file://{}", path.display());
    let ctx: Option<&gtk4::gio::AppLaunchContext> = None;
    if let Err(e) = gtk4::gio::AppInfo::launch_default_for_uri(&uri, ctx) {
        eprintln!("[settings] failed to open {uri}: {e}");
    }
}

/// The "⋮" overflow menu (Open save folder, Settings), used by both pages'
/// toolbars. Each call builds its own MenuButton/Popover/button instances --
/// GTK widgets can only have one parent, so the same physical widget can't
/// live in both the catalogue's and viewer's toolbars -- but both instances
/// call the same shell-based actions (`open_save_folder`,
/// `settings::open_settings`) so the two can't drift out of sync in behavior
/// even though they're separate widgets.
pub fn build_overflow_menu(shell: &Rc<Shell>, extra_buttons: &[gtk4::Button]) -> gtk4::MenuButton {
    let save_folder_button = gtk4::Button::with_label("Open save folder");
    let settings_button = gtk4::Button::with_label("Settings");

    let popover_box = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    for button in extra_buttons {
        popover_box.append(button);
    }
    popover_box.append(&save_folder_button);
    popover_box.append(&settings_button);

    let popover = gtk4::Popover::new();
    popover.set_child(Some(&popover_box));

    // Callers wire their own click behaviour on `extra_buttons` separately;
    // this just adds the same "close the popover after clicking" behaviour
    // the built-in buttons below get, since GTK allows multiple handlers on
    // one "clicked" signal.
    for button in extra_buttons {
        let popover = popover.clone();
        button.connect_clicked(move |_| popover.popdown());
    }

    save_folder_button.connect_clicked({
        let shell = shell.clone();
        let popover = popover.clone();
        move |_| {
            popover.popdown();
            open_save_folder(&shell);
        }
    });
    settings_button.connect_clicked({
        let shell = shell.clone();
        let popover = popover.clone();
        move |_| {
            popover.popdown();
            settings::open_settings(&shell);
        }
    });

    let menu_button = gtk4::MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_tooltip_text(Some("More"));
    menu_button.set_popover(Some(&popover));
    menu_button
}

/// A labelled, editable list widget: existing entries with a remove button
/// each, plus an add row. Shared by catalogue.rs's pin/hide word lists and
/// settings.rs's board list -- the third occurrence of this exact pattern,
/// which is what justified generalizing it out of catalogue.rs.
/// The two item-container styles `build_editable_list_widget` can render
/// into: a plain one-per-row list (unchanged original behaviour, still used
/// by Settings' board list), or a wrapping "chip" flow (filter word lists).
/// Both `gtk4::ListBox` and `gtk4::FlowBox` expose the same-shaped
/// `append`/`remove`/`first_child` calls, just not through a shared trait,
/// so this is a small concrete adapter rather than a generic abstraction.
enum ItemsContainer {
    List(gtk4::ListBox),
    Flow(gtk4::FlowBox),
}

impl ItemsContainer {
    fn widget(&self) -> gtk4::Widget {
        match self {
            ItemsContainer::List(w) => w.clone().upcast(),
            ItemsContainer::Flow(w) => w.clone().upcast(),
        }
    }

    fn clear(&self) {
        let widget = self.widget();
        while let Some(child) = widget.first_child() {
            match self {
                ItemsContainer::List(w) => w.remove(&child),
                ItemsContainer::Flow(w) => w.remove(&child),
            }
        }
    }

    fn add(&self, child: &impl IsA<gtk4::Widget>) {
        match self {
            ItemsContainer::List(w) => w.append(child),
            ItemsContainer::Flow(w) => w.append(child),
        }
    }
}

pub fn build_editable_list_widget(
    title: &str,
    placeholder: &str,
    flow_layout: bool,
    get_items: impl Fn() -> Vec<String> + 'static,
    add_item: impl Fn(String) + 'static,
    remove_item: impl Fn(&str) + 'static,
    on_change: Rc<dyn Fn()>,
) -> gtk4::Widget {
    let section = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let title_label = gtk4::Label::new(None);
    title_label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
    title_label.set_xalign(0.0);
    section.append(&title_label);

    let items_container = if flow_layout {
        let flow_box = gtk4::FlowBox::new();
        flow_box.set_selection_mode(gtk4::SelectionMode::None);
        // Chips flow left-to-right and wrap; rows would otherwise all be
        // stretched to the container's full width by default.
        flow_box.set_max_children_per_line(u32::MAX);
        // Without this, GTK sizes every chip's cell to match the widest one
        // in the box, so a short word like "bbc" would render as wide as
        // "favorite porn" sitting next to it.
        flow_box.set_homogeneous(false);
        flow_box.set_row_spacing(4);
        flow_box.set_column_spacing(4);
        ItemsContainer::Flow(flow_box)
    } else {
        let list_box = gtk4::ListBox::new();
        // A plain display list with per-row remove buttons, not something a
        // row itself should be selectable/highlightable for.
        list_box.set_selection_mode(gtk4::SelectionMode::None);
        ItemsContainer::List(list_box)
    };
    section.append(&items_container.widget());

    let get_items: Rc<dyn Fn() -> Vec<String>> = Rc::new(get_items);
    let add_item: Rc<dyn Fn(String)> = Rc::new(add_item);
    let remove_item: Rc<dyn Fn(&str)> = Rc::new(remove_item);

    let rebuild_cell: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    {
        let get_items = get_items.clone();
        let remove_item = remove_item.clone();
        let on_change = on_change.clone();
        let rebuild_cell_inner = rebuild_cell.clone();
        let rebuild = move || {
            items_container.clear();
            for item in get_items() {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                row.set_margin_top(2);
                row.set_margin_bottom(2);
                if flow_layout {
                    row.add_css_class("filter-chip");
                    // Without this, the chip still stretches to fill
                    // whatever cell width FlowBox hands it, even with
                    // homogeneous(false) set above.
                    row.set_halign(gtk4::Align::Start);
                }
                let label = gtk4::Label::new(Some(&item));
                label.set_xalign(0.0);
                label.set_hexpand(!flow_layout);
                let remove_button = gtk4::Button::with_label("×");
                remove_button.add_css_class("flat");
                {
                    let remove_item = remove_item.clone();
                    let on_change = on_change.clone();
                    let rebuild_cell = rebuild_cell_inner.clone();
                    let item = item.clone();
                    remove_button.connect_clicked(move |_| {
                        remove_item(&item);
                        on_change();
                        if let Some(f) = rebuild_cell.borrow().as_ref() {
                            f();
                        }
                    });
                }
                row.append(&label);
                row.append(&remove_button);
                items_container.add(&row);
            }
        };
        *rebuild_cell.borrow_mut() = Some(Rc::new(rebuild));
    }
    if let Some(f) = rebuild_cell.borrow().as_ref() {
        f();
    }

    let entry_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    let entry = gtk4::Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some(placeholder));
    let add_button = gtk4::Button::with_label("Add");
    entry_row.append(&entry);
    entry_row.append(&add_button);
    section.append(&entry_row);

    let add_entry_text: Rc<dyn Fn(&gtk4::Entry)> = {
        let add_item = add_item.clone();
        let on_change = on_change.clone();
        let rebuild_cell = rebuild_cell.clone();
        Rc::new(move |entry: &gtk4::Entry| {
            let text = entry.text().trim().to_string();
            if text.is_empty() {
                return;
            }
            // "|" and "," both split into multiple entries added at once
            // (e.g. "bbc | bbw, bbws") -- each piece is independently OR-
            // matched already (filters::find_matches), so this is purely an
            // input-parsing convenience, not a new relationship between them.
            for piece in text.split(['|', ',']) {
                let piece = piece.trim();
                if !piece.is_empty() {
                    add_item(piece.to_string());
                }
            }
            entry.set_text("");
            on_change();
            if let Some(f) = rebuild_cell.borrow().as_ref() {
                f();
            }
        })
    };
    {
        let add_entry_text = add_entry_text.clone();
        let entry = entry.clone();
        add_button.connect_clicked(move |_| add_entry_text(&entry));
    }
    entry.connect_activate(move |e| add_entry_text(e));

    section.upcast()
}

const DARK_THEME_CSS: &str = "
window, .background {
    background-color: #2b2b2b;
    color: #e8e8e8;
}
label { color: #e8e8e8; }
.dim-label { color: #a0a0a0; }
button, entry, textview, list, row, scale trough, scrolledwindow, .view {
    background-color: #383838;
    color: #e8e8e8;
}
entry, textview, .view { background-color: #202020; }
button:hover { background-color: #484848; }
button.suggested-action { background-color: #3584e4; color: #ffffff; }
button:checked { background-color: #3584e4; color: #ffffff; }
row:selected, list row:selected { background-color: #3584e4; color: #ffffff; }
scale trough { background-color: #202020; min-height: 6px; }
scale highlight { background-color: #3584e4; }
scale slider { background-color: #cccccc; }
row.pinned-row {
    background-color: #3a3010;
    border-left: 3px solid #d4a017;
}
row.pinned-row:selected {
    background-color: #4a3d14;
}
.filter-chip {
    background-color: #3a3a3a;
    border: 1px solid #4a4a4a;
    border-radius: 999px;
    padding: 2px 4px 2px 10px;
}
";
