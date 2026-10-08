//! The winit backend: runs xrcomp as a single window inside an existing Wayland
//! or X11 session. Like the x11 backend this is meant for development and
//! testing; the udev backend is what you want for real, everyday use.

use std::time::Duration;

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        egl::EGLDevice,
        renderer::{ImportDma, ImportEgl, ImportMemWl, damage::OutputDamageTracker, glow::GlowRenderer},
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{calloop::EventLoop, wayland_server::Display},
    utils::{Scale, Transform},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};
use tracing::{error, info, warn};

use crate::{
    render::{CLEAR_COLOR, Cursor, output_elements},
    scene::Scene,
    state::{Backend, State},
};

pub const OUTPUT_NAME: &str = "winit";

pub struct WinitData {
    backend: WinitGraphicsBackend<GlowRenderer>,
    damage_tracker: OutputDamageTracker,
    dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    cursor: Cursor,
    scene: Scene,
}

impl Backend for WinitData {
    fn seat_name(&self) -> String {
        "winit".to_owned()
    }
}

impl DmabufHandler for State<WinitData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self
            .backend_data
            .backend
            .renderer()
            .import_dmabuf(&dmabuf, None)
            .is_ok()
        {
            let _ = notifier.successful::<State<WinitData>>();
        } else {
            notifier.failed();
        }
    }
}

pub fn run_winit() {
    let mut event_loop: EventLoop<'static, State<WinitData>> = EventLoop::try_new().unwrap();
    let display: Display<State<WinitData>> = Display::new().unwrap();
    let display_handle = display.handle();

    let (mut backend, winit_loop) =
        winit::init::<GlowRenderer>().expect("Failed to initialize winit backend");

    if backend.renderer().bind_wl_display(&display_handle).is_ok() {
        info!("EGL hardware-acceleration enabled");
    }

    // Advertise dmabuf v4 (with feedback) when we can find the render node, otherwise fall
    // back to v3. Mesa's EGL needs either v4 or wl_drm (from `bind_wl_display` above).
    let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .and_then(|device| device.try_get_render_node());
    let dmabuf_formats = backend.renderer().dmabuf_formats();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = match render_node {
        Ok(Some(node)) => {
            let feedback = DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats)
                .build()
                .unwrap();
            dmabuf_state.create_global_with_default_feedback::<State<WinitData>>(&display_handle, &feedback)
        }
        Ok(None) => {
            warn!("Failed to query render node, dmabuf will use v3");
            dmabuf_state.create_global::<State<WinitData>>(&display_handle, dmabuf_formats)
        }
        Err(err) => {
            warn!(?err, "Failed to get EGL device for display, dmabuf will use v3");
            dmabuf_state.create_global::<State<WinitData>>(&display_handle, dmabuf_formats)
        }
    };

    // We draw our own software cursor (see `render::Cursor`), so hide the host's.
    backend.window().set_cursor_visible(false);

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "xrcomp".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    output.create_global::<State<WinitData>>(&display_handle);
    // winit's EGL surface is y-flipped relative to what the renderer produces.
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);

    let data = WinitData {
        backend,
        damage_tracker,
        dmabuf_state,
        _dmabuf_global: dmabuf_global,
        cursor: Cursor::new(),
        scene: Scene::new(),
    };

    let mut state = State::new(&mut event_loop, display, data);
    let shm_formats = state.backend_data.backend.renderer().shm_formats();
    state.shm_state.update_formats(shm_formats);
    state.space.map_output(&output, (0, 0));

    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    let output_clone = output.clone();
    event_loop
        .handle()
        .insert_source(winit_loop, move |event, _, state| match event {
            WinitEvent::CloseRequested => {
                state.running = false;
            }
            WinitEvent::Resized { size, .. } => {
                let mode = Mode { size, refresh: 60_000 };
                if let Some(current) = output_clone.current_mode() {
                    output_clone.delete_mode(current);
                }
                output_clone.change_current_state(Some(mode), None, None, None);
                output_clone.set_preferred(mode);
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw | WinitEvent::Focus(_) => {}
        })
        .expect("Failed to insert winit backend into event loop");

    info!("Initialization completed, starting the main loop.");

    while state.running {
        render_frame(&mut state, &output);

        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut state);
        if result.is_err() {
            state.running = false;
        } else {
            state.space.refresh();
            state.popups.cleanup();
            let _ = state.display_handle.flush_clients();
        }
    }
}

/// Renders a frame and returns whether anything was damaged and submitted.
fn render_frame(state: &mut State<WinitData>, output: &Output) -> bool {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let cursor_pos = state
        .pointer()
        .current_location()
        .to_physical(scale)
        .to_i32_round();

    let WinitData {
        backend,
        damage_tracker,
        cursor,
        scene,
        ..
    } = &mut state.backend_data;

    // EGL only reports the buffer age of the surface that is current. The window surface
    // stays current from the previous frame unless something since then rendered into a
    // texture or imported a buffer (which leaves the context current without a surface,
    // as `scene.prepare` below does), so query it first and fall back to a full redraw.
    let age = if backend.egl_surface().is_current() {
        backend.buffer_age().unwrap_or(0)
    } else {
        0
    };

    let cursor_element = cursor.render_element(cursor_pos, scale);
    let scene_element = scene.prepare(backend.renderer(), &state.space, output);
    let elements = output_elements(cursor_element, scene_element);

    let render_res = {
        let (renderer, mut fb) = match backend.bind() {
            Ok(bound) => bound,
            Err(err) => {
                error!("Error while binding winit backend: {}", err);
                return false;
            }
        };

        damage_tracker.render_output(renderer, &mut fb, age, &elements, CLEAR_COLOR)
    };

    match render_res {
        Ok(res) => {
            let rendered = if let Some(damage) = res.damage {
                if let Err(err) = backend.submit(Some(damage)) {
                    warn!("Failed to submit buffer: {}", err);
                }
                true
            } else {
                false
            };

            let now = state.start_time.elapsed();
            state.space.elements().for_each(|window| {
                window.send_frame(output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
            });

            rendered
        }
        Err(err) => {
            error!("Rendering error: {}", err);
            false
        }
    }
}
