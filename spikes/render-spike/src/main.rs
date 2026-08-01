// render-spike v0.1.0
//
// Throwaway program for project.md §18.1.1: confirm libmpv can render
// reliably inside a GTK4 widget on this system before any real application
// code is written. Not part of the 1cheri application itself.
//
// Manual test checklist (project.md §19.1):
//   - window resizing
//   - hardware decoding
//   - multiple sequential WebM files (press N to load the next path given on argv)
//   - fullscreen transitions (F11 or F)
//   - audio mute (M)
//   - player shutdown (close the window / Q)
//   - rapid media switching (press N repeatedly)

use gtk4::prelude::*;
use gtk4::{gdk, glib};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use std::cell::RefCell;
use std::env;
use std::ffi::{c_void, CString};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

fn print_help() {
    println!("render-spike {VERSION}");
    println!("Throwaway libmpv-in-GTK4 render spike for the 1cheri project.");
    println!();
    println!("Usage: render-spike [OPTIONS] <FILE> [FILE...]");
    println!();
    println!("Arguments:");
    println!("  <FILE>...  One or more local video files to play. Press N to advance.");
    println!();
    println!("Options:");
    println!("  -h, --help     Print this help and exit");
    println!("  -v, --version  Print version and exit");
    println!();
    println!("Keys: Space=pause  M=mute  R=restart  F=fullscreen  N=next file  Q=quit");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-v" || a == "--version") {
        println!("render-spike {VERSION}");
        return ExitCode::SUCCESS;
    }
    if args.is_empty() {
        eprintln!("error: no input file given\n");
        print_help();
        return ExitCode::FAILURE;
    }

    let files: Rc<Vec<String>> = Rc::new(args);

    let app = gtk4::Application::builder()
        .application_id("org.cheri.render-spike")
        .build();

    app.connect_activate(move |app| build_ui(app, files.clone()));

    let empty: [&str; 0] = [];
    app.run_with_args(&empty);
    ExitCode::SUCCESS
}

/// Loads real GL function addresses for already-linked libepoxy symbols.
/// Safe to call more than once; epoxy's `load_with` just repopulates its
/// internal dispatch table.
fn load_epoxy() {
    epoxy::load_with(|name| unsafe {
        match CString::new(name) {
            Ok(cname) => libc::dlsym(libc::RTLD_DEFAULT, cname.as_ptr()) as *const c_void,
            Err(_) => std::ptr::null(),
        }
    });
}

fn mpv_get_proc_address(_ctx: &(), name: &str) -> *mut c_void {
    epoxy::get_proc_addr(name) as *mut c_void
}

struct Player {
    mpv: Mpv,
    render_context: RenderContext<'static>,
}

fn build_ui(app: &gtk4::Application, files: Rc<Vec<String>>) {
    let gl_area = gtk4::GLArea::new();
    gl_area.set_has_depth_buffer(false);
    gl_area.set_has_stencil_buffer(false);
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .title("render-spike")
        .default_width(960)
        .default_height(540)
        .child(&gl_area)
        .build();

    let player: Rc<RefCell<Option<Player>>> = Rc::new(RefCell::new(None));
    let current_index = Rc::new(RefCell::new(0usize));

    // SAFETY: the Mpv and RenderContext created here only ever get used while
    // this GLArea's GL context is current, which GTK guarantees for the
    // duration of `realize` and `render` signal emissions on the widget that
    // owns the context. We never move them across threads.
    {
        let player = player.clone();
        let files = files.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            if let Some(err) = area.error() {
                eprintln!("GL area failed to realize: {err}");
                return;
            }

            load_epoxy();

            // GTK's own init calls setlocale(LC_ALL, "") and resets
            // LC_NUMERIC away from "C", so this has to be redone right
            // before mpv_create() rather than once at process startup.
            unsafe {
                libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
            }

            let mpv = Mpv::with_initializer(|init| {
                init.set_property("vo", "libmpv")?;
                init.set_property("hwdec", "auto")?;
                init.set_property("terminal", "no")?;
                init.set_property("keep-open", "yes")?;
                Ok(())
            })
            .expect("failed to create Mpv instance");
            mpv.set_property("mute", true).ok();
            mpv.set_property("volume", 70i64).ok();

            let render_context = mpv
                .create_render_context(vec![
                    RenderParam::ApiType(RenderParamApiType::OpenGl),
                    RenderParam::InitParams(OpenGLInitParams {
                        get_proc_address: mpv_get_proc_address,
                        ctx: (),
                    }),
                ])
                .expect("failed to create mpv render context");

            // Leak the borrow-checker lifetime: this render context lives as
            // long as `Player`, which lives as long as the GLArea (spike-only
            // shortcut; the real app should scope this more carefully).
            let render_context: RenderContext<'static> = unsafe { std::mem::transmute(render_context) };

            *player.borrow_mut() = Some(Player { mpv, render_context });

            if let Some(first) = files.first() {
                if let Some(p) = player.borrow().as_ref() {
                    p.mpv.command("loadfile", &[first, "replace"]).ok();
                }
            }
        });
    }

    {
        let player = player.clone();
        gl_area.connect_render(move |_area, _ctx| {
            if let Some(p) = player.borrow().as_ref() {
                let mut fbo: i32 = 0;
                unsafe { epoxy::GetIntegerv(GL_FRAMEBUFFER_BINDING, &mut fbo) };

                let width = _area.width() * _area.scale_factor();
                let height = _area.height() * _area.scale_factor();
                if width > 0 && height > 0 {
                    if let Err(e) = p.render_context.render::<()>(fbo, width, height, true) {
                        eprintln!("mpv render error: {e}");
                    }
                }
            }
            glib::Propagation::Stop
        });
    }

    // Poll mpv on a fixed tick instead of wiring cross-thread wakeup
    // callbacks: simplest possible thing that proves embedding works, and
    // this is a throwaway spike, not the real playback loop.
    {
        let player = player.clone();
        let gl_area = gl_area.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if let Some(p) = player.borrow().as_ref() {
                while let Some(event) = p.mpv.wait_event(0.0) {
                    match event {
                        Ok(libmpv2::events::Event::EndFile(_)) => {
                            println!("[mpv] end-file");
                        }
                        Ok(libmpv2::events::Event::FileLoaded) => {
                            println!("[mpv] file-loaded");
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("[mpv] event error: {e}"),
                    }
                }

                if let Ok(update) = p.render_context.update() {
                    if update & libmpv2::render::mpv_render_update::Frame != 0 {
                        gl_area.queue_render();
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // Keyboard controls, per project.md §10 (subset relevant to this spike).
    {
        let player = player.clone();
        let files = files.clone();
        let current_index = current_index.clone();
        let window_for_key = window.clone();
        let key_controller = gtk4::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_ctrl, key, _code, _state| {
            let player_ref = player.borrow();
            let Some(p) = player_ref.as_ref() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::q | gdk::Key::Q => {
                    window_for_key.close();
                }
                gdk::Key::space => {
                    let paused: bool = p.mpv.get_property("pause").unwrap_or(false);
                    p.mpv.set_property("pause", !paused).ok();
                }
                gdk::Key::m | gdk::Key::M => {
                    let muted: bool = p.mpv.get_property("mute").unwrap_or(false);
                    p.mpv.set_property("mute", !muted).ok();
                    println!("[mpv] mute = {}", !muted);
                }
                gdk::Key::r | gdk::Key::R => {
                    p.mpv.command("seek", &["0", "absolute"]).ok();
                }
                gdk::Key::f | gdk::Key::F | gdk::Key::F11 => {
                    if window_for_key.is_fullscreen() {
                        window_for_key.unfullscreen();
                    } else {
                        window_for_key.fullscreen();
                    }
                }
                gdk::Key::n | gdk::Key::N => {
                    let mut idx = current_index.borrow_mut();
                    if !files.is_empty() {
                        *idx = (*idx + 1) % files.len();
                        let path = &files[*idx];
                        println!("[mpv] loading next file: {path}");
                        p.mpv.command("loadfile", &[path.as_str(), "replace"]).ok();
                    }
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        gl_area.add_controller(key_controller);
    }

    window.present();
}
