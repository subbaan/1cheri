// Embeds libmpv's OpenGL render API inside a GTK4 GLArea. This is the
// same approach validated in spikes/render-spike; see that crate's
// comments for the reasoning behind each step (locale fix, epoxy loading,
// polling instead of cross-thread callbacks).

use gtk4::prelude::*;
use gtk4::glib;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use libmpv2::Mpv;
use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::rc::Rc;
use std::time::Duration;

const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

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

pub struct Player {
    // Field order matters: Rust drops struct fields in declaration order
    // (top to bottom), and mpv's render API requires the render context to
    // be freed *before* the core is destroyed. With `mpv` listed first,
    // dropping a `Player` called `mpv_destroy` before
    // `mpv_render_context_free`, a use-after-free that reliably aborted the
    // process as soon as a viewer session was torn down (e.g. pressing
    // Escape/B to go back to the catalogue).
    render_context: RenderContext<'static>,
    mpv: Mpv,
}

impl Player {
    pub fn load(&self, path: &str) {
        if let Err(e) = self.mpv.command("loadfile", &[path, "replace"]) {
            eprintln!("[player] failed to load {path}: {e}");
        }
    }

    pub fn is_muted(&self) -> bool {
        self.mpv.get_property("mute").unwrap_or(false)
    }

    pub fn set_muted(&self, muted: bool) {
        self.mpv.set_property("mute", muted).ok();
    }

    pub fn is_paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }

    pub fn set_paused(&self, paused: bool) {
        self.mpv.set_property("pause", paused).ok();
    }

    pub fn set_volume(&self, volume: i64) {
        self.mpv.set_property("volume", volume).ok();
    }

    pub fn restart(&self) {
        self.mpv.command("seek", &["0", "absolute"]).ok();
    }

    /// Playback position in seconds, if a file with known position is loaded.
    pub fn position(&self) -> Option<f64> {
        self.mpv.get_property("time-pos").ok()
    }

    /// Total duration in seconds, if known (absent for some live/odd streams).
    pub fn duration(&self) -> Option<f64> {
        self.mpv.get_property("duration").ok()
    }

    pub fn seek_absolute(&self, seconds: f64) {
        self.mpv.command("seek", &[&seconds.to_string(), "absolute"]).ok();
    }
}

pub type PlayerHandle = Rc<RefCell<Option<Player>>>;

/// Wires up a GLArea to render mpv's output. Returns a handle the caller uses
/// to load files and control playback once the underlying GL context has
/// been realised (which happens asynchronously, shortly after the window is
/// shown).
pub fn attach_player(
    gl_area: &gtk4::GLArea,
    initial_muted: bool,
    initial_volume: i64,
    initial_loop: bool,
    on_error: impl Fn(String) + 'static,
) -> (PlayerHandle, glib::SourceId) {
    gl_area.set_has_depth_buffer(false);
    gl_area.set_has_stencil_buffer(false);

    let player: PlayerHandle = Rc::new(RefCell::new(None));

    {
        let player = player.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            if let Some(err) = area.error() {
                eprintln!("[player] GL area failed to realize: {err}");
                return;
            }

            load_epoxy();

            // GTK's own init resets LC_NUMERIC away from "C" after it calls
            // setlocale(LC_ALL, ""); mpv requires "C" and mpv_create() fails
            // otherwise, so this must be redone here, not just at startup.
            unsafe {
                libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
            }

            // On a non-Nvidia system, `hwdec=auto` (and `auto-safe`, tried
            // and reverted -- no difference) probes CUDA and fails, printing
            // "Cannot load libcuda.so.1" straight to stderr. `terminal=no`
            // below doesn't reach it: it comes from the CUDA loader's own
            // dlopen stub, not mpv's log router, so it can't be quieted via
            // any mpv property. Harmless (mpv falls through to the decoder
            // that actually works), left as a known, one-line-per-launch
            // cosmetic wart rather than adding process-wide stderr
            // redirection just to hide it.
            let mpv = match Mpv::with_initializer(|init| {
                init.set_property("vo", "libmpv")?;
                init.set_property("hwdec", "auto")?;
                init.set_property("terminal", "no")?;
                init.set_property("keep-open", "yes")?;
                init.set_property("loop-file", if initial_loop { "inf" } else { "no" })?;
                Ok(())
            }) {
                Ok(mpv) => mpv,
                Err(e) => {
                    eprintln!("[player] failed to create mpv instance: {e}");
                    return;
                }
            };
            mpv.set_property("mute", initial_muted).ok();
            mpv.set_property("volume", initial_volume).ok();

            let render_context = match mpv.create_render_context(vec![
                RenderParam::ApiType(RenderParamApiType::OpenGl),
                RenderParam::InitParams(OpenGLInitParams {
                    get_proc_address: mpv_get_proc_address,
                    ctx: (),
                }),
            ]) {
                Ok(rc) => rc,
                Err(e) => {
                    eprintln!("[player] failed to create render context: {e}");
                    return;
                }
            };
            // SAFETY: this RenderContext is only ever used from within this
            // GLArea's realize/render signal handlers, all on the GTK main
            // thread, for as long as the widget (and thus `player`) is alive.
            let render_context: RenderContext<'static> = unsafe { std::mem::transmute(render_context) };

            *player.borrow_mut() = Some(Player { mpv, render_context });
        });
    }

    {
        let player = player.clone();
        gl_area.connect_render(move |area, _ctx| {
            if let Some(p) = player.borrow().as_ref() {
                let mut fbo: i32 = 0;
                unsafe { epoxy::GetIntegerv(GL_FRAMEBUFFER_BINDING, &mut fbo) };

                let width = area.width() * area.scale_factor();
                let height = area.height() * area.scale_factor();
                if width > 0 && height > 0 {
                    if let Err(e) = p.render_context.render::<()>(fbo, width, height, true) {
                        eprintln!("[player] render error: {e}");
                    }
                }
            }
            glib::Propagation::Stop
        });
    }

    // Poll instead of wiring mpv's cross-thread update/wakeup callbacks: the
    // render spike proved this is simple, safe, and plenty fast (~60Hz) for
    // a desktop viewer.
    //
    // The caller must remove this SourceId when the GLArea is torn down
    // (e.g. opening a different thread): a glib timeout keeps firing
    // regardless of whether the widgets it references are still on screen,
    // and would otherwise leak an mpv instance per opened thread.
    let poll_source = {
        let player = player.clone();
        let gl_area = gl_area.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if let Some(p) = player.borrow().as_ref() {
                while let Some(event) = p.mpv.wait_event(0.0) {
                    match event {
                        Ok(_) => {}
                        // Reached for a genuine load/playback failure (corrupt
                        // file, unsupported codec, dead link) -- mpv reports
                        // this as an end-of-file event with a non-zero error
                        // code, which libmpv2 surfaces here as `Err` rather
                        // than as `Ok(Event::EndFile(..))`. Previously this
                        // only went to stderr, so a failed item just looked
                        // like playback silently not starting.
                        Err(e) => {
                            eprintln!("[mpv] event error: {e}");
                            on_error(e.to_string());
                        }
                    }
                }
                if let Ok(update) = p.render_context.update() {
                    if update & libmpv2::render::mpv_render_update::Frame != 0 {
                        gl_area.queue_render();
                    }
                }
            }
            glib::ControlFlow::Continue
        })
    };

    (player, poll_source)
}
