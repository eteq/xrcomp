//! The X11 backend: runs xrcomp as a single window inside an existing X11 (or
//! Xwayland) session. This is the backend used for development and testing;
//! the udev backend is what you want for real, everyday use.

use std::time::Duration;

use smithay::{
    backend::{
        allocator::{
            dmabuf::{Dmabuf, DmabufAllocator},
            gbm::{GbmAllocator, GbmBufferFlags},
        },
        egl::{EGLContext, EGLDisplay},
        renderer::{Bind, ImportDma, ImportEgl, ImportMemWl, damage::OutputDamageTracker, glow::GlowRenderer},
        x11::{WindowBuilder, X11Backend, X11Event, X11Surface},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{calloop::EventLoop, gbm, wayland_server::Display},
    utils::{DeviceFd, Scale},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};
use tracing::{error, info, warn};

use crate::{
    render::{CLEAR_COLOR, Cursor, output_elements},
    scene::Scene,
    state::{Backend, State},
};

pub const OUTPUT_NAME: &str = "x11";

pub struct X11Data {
    surface: X11Surface,
    renderer: GlowRenderer,
    damage_tracker: OutputDamageTracker,
    dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    cursor: Cursor,
    scene: Scene,
}

impl Backend for X11Data {
    fn seat_name(&self) -> String {
        "x11".to_owned()
    }
}

impl DmabufHandler for State<X11Data> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend_data.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<State<X11Data>>();
        } else {
            notifier.failed();
        }
    }
}

pub fn run_x11() {
    let mut event_loop: EventLoop<'static, State<X11Data>> = EventLoop::try_new().unwrap();
    let display: Display<State<X11Data>> = Display::new().unwrap();
    let display_handle = display.handle();

    let backend = X11Backend::new().expect("Failed to initialize X11 backend");
    let handle = backend.handle();

    let (node, fd) = handle
        .drm_node()
        .expect("Could not get DRM node used by the X server");

    let device = gbm::Device::new(DeviceFd::from(fd)).expect("Failed to create gbm device");
    let egl = unsafe { EGLDisplay::new(device.clone()).expect("Failed to create EGLDisplay") };
    let context = EGLContext::new(&egl).expect("Failed to create EGLContext");

    let window = WindowBuilder::new()
        .title("xrcomp")
        .build(&handle)
        .expect("Failed to create first window");

    let surface = handle
        .create_surface(
            &window,
            DmabufAllocator(GbmAllocator::new(device, GbmBufferFlags::RENDERING)),
            context.dmabuf_render_formats().iter().map(|format| format.modifier),
        )
        .expect("Failed to create X11 surface");

    let mut renderer = unsafe { GlowRenderer::new(context) }.expect("Failed to initialize renderer");
    if renderer.bind_wl_display(&display_handle).is_ok() {
        info!("EGL hardware-acceleration enabled");
    }

    let dmabuf_formats = renderer.dmabuf_formats();
    let dmabuf_default_feedback = DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = dmabuf_state
        .create_global_with_default_feedback::<State<X11Data>>(&display_handle, &dmabuf_default_feedback);

    let size = {
        let s = window.size();
        (s.w as i32, s.h as i32).into()
    };
    let mode = Mode { size, refresh: 60_000 };

    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "xrcomp".into(),
            model: "X11".into(),
            serial_number: "Unknown".into(),
        },
    );
    output.create_global::<State<X11Data>>(&display_handle);
    output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
    output.set_preferred(mode);

    let damage_tracker = OutputDamageTracker::from_output(&output);

    let data = X11Data {
        surface,
        renderer,
        damage_tracker,
        dmabuf_state,
        _dmabuf_global: dmabuf_global,
        cursor: Cursor::new(),
        scene: Scene::new(),
    };

    let mut state = State::new(&mut event_loop, display, data);
    state
        .shm_state
        .update_formats(state.backend_data.renderer.shm_formats());
    state.space.map_output(&output, (0, 0));

    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    let output_clone = output.clone();
    event_loop
        .handle()
        .insert_source(backend, move |event, _, state| match event {
            X11Event::CloseRequested { .. } => {
                state.running = false;
            }
            X11Event::Resized { new_size, .. } => {
                let size = (new_size.w as i32, new_size.h as i32).into();
                let mode = Mode { size, refresh: 60_000 };
                if let Some(current) = output_clone.current_mode() {
                    output_clone.delete_mode(current);
                }
                output_clone.change_current_state(Some(mode), None, None, None);
                output_clone.set_preferred(mode);
            }
            X11Event::PresentCompleted { .. } | X11Event::Refresh { .. } => {}
            X11Event::Input { event, .. } => state.process_input_event(event),
            _ => {}
        })
        .expect("Failed to insert X11 Backend into event loop");

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

/// Renders a frame and returns whether it was submitted successfully.
fn render_frame(state: &mut State<X11Data>, output: &Output) -> bool {
    let scale = Scale::from(output.current_scale().fractional_scale());
    let cursor_pos = state
        .pointer()
        .current_location()
        .to_physical(scale)
        .to_i32_round();

    let backend_data = &mut state.backend_data;

    let scene_element = backend_data
        .scene
        .prepare(&mut backend_data.renderer, &state.space, output);

    let (mut buffer, age) = match backend_data.surface.buffer() {
        Ok(b) => b,
        Err(err) => {
            error!("Failed to get X11 buffer: {}", err);
            return false;
        }
    };
    let mut fb = match backend_data.renderer.bind(&mut buffer) {
        Ok(fb) => fb,
        Err(err) => {
            error!("Error while binding buffer: {}", err);
            return false;
        }
    };

    let cursor_element = backend_data.cursor.render_element(cursor_pos, scale);

    let elements = output_elements(cursor_element, scene_element);

    let render_res = backend_data.damage_tracker.render_output(
        &mut backend_data.renderer,
        &mut fb,
        age as usize,
        &elements,
        CLEAR_COLOR,
    );

    match render_res {
        Ok(res) => {
            let rendered = res.damage.is_some();
            if let Err(err) = backend_data.surface.submit() {
                backend_data.surface.reset_buffers();
                warn!("Failed to submit buffer: {}. Retrying", err);
            }

            let now = state.start_time.elapsed();
            state.space.elements().for_each(|window| {
                window.send_frame(output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
            });

            rendered
        }
        Err(err) => {
            backend_data.surface.reset_buffers();
            error!("Rendering error: {}", err);
            false
        }
    }
}
