//! The udev/DRM/libinput/libseat backend: runs xrcomp on a raw TTY, driving
//! the GPU directly via KMS and reading input devices via libinput. This is
//! the backend intended for normal, everyday use.
//!
//! This is a deliberately minimal single-GPU implementation: no multi-GPU
//! rendering, no DRM lease support, no render-node fallback path. If you
//! need any of that, look at Smithay's `anvil` example, which this is
//! loosely modeled on.

use std::{collections::HashMap, time::Duration};

use smithay::{
    backend::{
        allocator::{Fourcc, gbm::GbmAllocator, gbm::GbmBufferFlags, gbm::GbmDevice},
        drm::{
            DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata, DrmNode,
            compositor::FrameFlags,
            exporter::gbm::{GbmFramebufferExporter, NodeFilter},
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{ImportDma, ImportEgl, ImportMemWl, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::space::space_render_elements,
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{
            Display,
            backend::GlobalId,
        },
    },
    utils::DeviceFd,
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use tracing::{error, info, warn};

use crate::{
    render::{CLEAR_COLOR, Cursor, OutputElement, output_scale},
    spawn_client,
    state::{Backend, State},
};

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type UdevDrmOutputManager = DrmOutputManager<Allocator, Exporter, (), DrmDeviceFd>;
type UdevDrmOutput = DrmOutput<Allocator, Exporter, (), DrmDeviceFd>;

struct SurfaceData {
    output: Output,
    global: GlobalId,
    drm_output: UdevDrmOutput,
}

pub struct UdevData {
    session: LibSeatSession,
    renderer: GlesRenderer,
    drm_output_manager: UdevDrmOutputManager,
    drm_scanner: DrmScanner,
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    dmabuf_state: DmabufState,
    _dmabuf_global: DmabufGlobal,
    cursor: Cursor,
}

impl Backend for UdevData {
    fn seat_name(&self) -> String {
        self.session.seat()
    }
}

impl DmabufHandler for State<UdevData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend_data.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<State<UdevData>>();
        } else {
            notifier.failed();
        }
    }
}

pub fn run_udev() {
    let mut event_loop: EventLoop<'static, State<UdevData>> = EventLoop::try_new().unwrap();
    let display: Display<State<UdevData>> = Display::new().unwrap();
    let display_handle = display.handle();

    let (mut session, session_notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            error!("Could not initialize a session: {}", err);
            return;
        }
    };
    let seat_name = session.seat();

    let primary_gpu_path = std::env::var("XRCOMP_DRM_DEVICE")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| primary_gpu(&seat_name).ok().flatten())
        .or_else(|| all_gpus(&seat_name).ok().and_then(|gpus| gpus.into_iter().next()))
        .expect("No GPU found");
    let primary_gpu =
        DrmNode::from_path(&primary_gpu_path).expect("Primary GPU device is not a DRM node");
    info!(?primary_gpu, "Using primary GPU");

    let fd = session
        .open(
            &primary_gpu_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .expect("Failed to open DRM device");
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true).expect("Failed to initialize DRM device");
    let gbm = GbmDevice::new(fd).expect("Failed to initialize GBM device");

    let egl_display = unsafe { EGLDisplay::new(gbm.clone()).expect("Failed to create EGLDisplay") };
    let egl_context = EGLContext::new(&egl_display).expect("Failed to create EGLContext");
    let mut renderer = unsafe { GlesRenderer::new(egl_context) }.expect("Failed to initialize renderer");
    if renderer.bind_wl_display(&display_handle).is_ok() {
        info!("EGL hardware-acceleration enabled");
    }

    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let framebuffer_exporter = GbmFramebufferExporter::new(gbm.clone(), NodeFilter::All);
    let color_formats = [Fourcc::Argb8888, Fourcc::Abgr8888];
    let render_formats = renderer.egl_context().dmabuf_render_formats().clone();

    let drm_output_manager = DrmOutputManager::new(
        drm,
        allocator,
        framebuffer_exporter,
        Some(gbm),
        color_formats,
        render_formats,
    );

    let dmabuf_formats = renderer.dmabuf_formats();
    let dmabuf_default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = dmabuf_state
        .create_global_with_default_feedback::<State<UdevData>>(&display_handle, &dmabuf_default_feedback);

    let data = UdevData {
        session,
        renderer,
        drm_output_manager,
        drm_scanner: DrmScanner::new(),
        surfaces: HashMap::new(),
        dmabuf_state,
        _dmabuf_global: dmabuf_global,
        cursor: Cursor::new(),
    };

    let mut state = State::new(&mut event_loop, display, data);
    state
        .shm_state
        .update_formats(state.backend_data.renderer.shm_formats());

    event_loop
        .handle()
        .insert_source(drm_notifier, move |event, metadata, state: &mut State<UdevData>| match event {
            DrmEvent::VBlank(crtc) => state.frame_finish(crtc, metadata),
            DrmEvent::Error(err) => error!("{:?}", err),
        })
        .unwrap();

    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context.udev_assign_seat(&seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, state| {
            state.process_input_event(event);
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(session_notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                info!("pausing session");
                libinput_context.suspend();
                state.backend_data.drm_output_manager.pause();
            }
            SessionEvent::ActivateSession => {
                info!("resuming session");
                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }
                if let Err(err) = state.backend_data.drm_output_manager.lock().activate(false) {
                    error!("Failed to activate drm backend: {:?}", err);
                }
                let crtcs: Vec<_> = state.backend_data.surfaces.keys().copied().collect();
                for crtc in crtcs {
                    state.render_surface(crtc);
                }
            }
        })
        .unwrap();

    let udev_backend = match UdevBackend::new(&seat_name) {
        Ok(backend) => backend,
        Err(err) => {
            error!("Failed to initialize udev backend: {:?}", err);
            return;
        }
    };

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, state| match event {
            UdevEvent::Changed { device_id } => {
                if DrmNode::from_dev_id(device_id).is_ok_and(|node| node == primary_gpu) {
                    state.scan_connectors();
                }
            }
            UdevEvent::Removed { device_id } => {
                if DrmNode::from_dev_id(device_id).is_ok_and(|node| node == primary_gpu) {
                    warn!("Primary GPU was removed, shutting down");
                    state.running = false;
                }
            }
            UdevEvent::Added { .. } => {
                // Hot-plugged secondary GPUs are not supported by this minimal backend.
            }
        })
        .unwrap();

    state.scan_connectors();

    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };
    spawn_client();

    info!("Initialization completed, starting the main loop.");

    while state.running {
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

impl State<UdevData> {
    fn scan_connectors(&mut self) {
        let scan_result = match self
            .backend_data
            .drm_scanner
            .scan_connectors(self.backend_data.drm_output_manager.device())
        {
            Ok(result) => result,
            Err(err) => {
                warn!("Failed to scan connectors: {:?}", err);
                return;
            }
        };

        for event in scan_result {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_connected(connector, crtc),
                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_disconnected(connector, crtc),
                _ => {}
            }
        }
    }

    fn connector_connected(&mut self, connector: connector::Info, crtc: crtc::Handle) {
        let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        info!(?crtc, "Setting up connector {}", output_name);

        let mode_id = connector
            .modes()
            .iter()
            .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(0);
        let Some(&drm_mode) = connector.modes().get(mode_id) else {
            warn!("Connector {} has no modes", output_name);
            return;
        };
        let wl_mode = WlMode::from(drm_mode);

        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name.clone(),
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: connector.subpixel().into(),
                make: "xrcomp".into(),
                model: "Unknown".into(),
                serial_number: "Unknown".into(),
            },
        );
        let global = output.create_global::<State<UdevData>>(&self.display_handle);
        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some((0, 0).into()));
        self.space.map_output(&output, (0, 0));

        let backend = &mut self.backend_data;
        let drm_output = match backend.drm_output_manager.lock().initialize_output::<_, OutputElement>(
            crtc,
            drm_mode,
            &[connector.handle()],
            &output,
            None,
            &mut backend.renderer,
            &DrmOutputRenderElements::default(),
        ) {
            Ok(drm_output) => drm_output,
            Err(err) => {
                warn!("Failed to initialize drm output for {}: {}", output_name, err);
                self.display_handle.remove_global::<State<UdevData>>(global);
                self.space.unmap_output(&output);
                return;
            }
        };

        backend.surfaces.insert(
            crtc,
            SurfaceData {
                output,
                global,
                drm_output,
            },
        );

        self.loop_handle.insert_idle(move |state| state.render_surface(crtc));
    }

    fn connector_disconnected(&mut self, _connector: connector::Info, crtc: crtc::Handle) {
        if let Some(surface) = self.backend_data.surfaces.remove(&crtc) {
            self.space.unmap_output(&surface.output);
            self.display_handle.remove_global::<State<UdevData>>(surface.global);
        }
    }

    fn frame_finish(&mut self, crtc: crtc::Handle, _metadata: &mut Option<DrmEventMetadata>) {
        let Some(surface) = self.backend_data.surfaces.get(&crtc) else {
            return;
        };
        if let Err(err) = surface.drm_output.frame_submitted() {
            warn!("Error marking frame as submitted: {:?}", err);
        }
        self.render_surface(crtc);
    }

    fn render_surface(&mut self, crtc: crtc::Handle) {
        let Some(surface) = self.backend_data.surfaces.get_mut(&crtc) else {
            return;
        };
        let output = surface.output.clone();
        let scale = output_scale(&output);
        let cursor_pos = self.pointer().current_location().to_physical(scale).to_i32_round();
        let cursor_element = self
            .backend_data
            .cursor
            .render_element(cursor_pos, scale);

        let space_elements =
            match space_render_elements(&mut self.backend_data.renderer, [&self.space], &output, 1.0) {
                Ok(elements) => elements,
                Err(err) => {
                    warn!("Failed to collect render elements: {:?}", err);
                    return;
                }
            };

        let elements: Vec<OutputElement> = space_elements
            .into_iter()
            .map(OutputElement::Space)
            .chain(std::iter::once(OutputElement::Cursor(cursor_element)))
            .collect();

        let Some(surface) = self.backend_data.surfaces.get_mut(&crtc) else {
            return;
        };
        let render_result = surface
            .drm_output
            .render_frame(&mut self.backend_data.renderer, &elements, CLEAR_COLOR, FrameFlags::DEFAULT);

        match render_result {
            Ok(res) if !res.is_empty => {
                if let Err(err) = surface.drm_output.queue_frame(()) {
                    warn!("Failed to queue frame: {:?}", err);
                    return;
                }
                let now = self.start_time.elapsed();
                self.space.elements().for_each(|window| {
                    window.send_frame(&output, now, Some(Duration::ZERO), |_, _| Some(output.clone()));
                });
            }
            Ok(_) => {
                // No damage: nothing was queued, so no vblank will arrive to drive the next
                // render. Check back shortly instead.
                self.loop_handle
                    .insert_source(Timer::from_duration(Duration::from_millis(16)), move |_, _, state| {
                        state.render_surface(crtc);
                        TimeoutAction::Drop
                    })
                    .ok();
            }
            Err(err) => {
                warn!("Rendering error: {:?}", err);
                if matches!(
                    err,
                    smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(
                        smithay::backend::drm::compositor::FrameError::DrmError(DrmError::DeviceInactive)
                    )
                ) {
                    // Session is inactive; ActivateSession will kick off rendering again.
                    return;
                }
                self.loop_handle
                    .insert_source(Timer::from_duration(Duration::from_millis(16)), move |_, _, state| {
                        state.render_surface(crtc);
                        TimeoutAction::Drop
                    })
                    .ok();
            }
        }
    }
}
