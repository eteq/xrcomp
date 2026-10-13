use interprocess::local_socket;
use interprocess::local_socket::{prelude::*, ListenerNonblockingMode, ListenerOptions};
use interprocess::os::unix::uds_local_socket::Stream as UdsStream;

use std::{
    ffi::OsString,
    io::{self, Read, Write, BufReader},
    sync::Arc,
};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState, pointer::PointerHandle},
    reexports::{
        calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use tracing::{warn, info};

//use crate::comms::process_message;


/// Backend-specific hooks that the shared compositor state needs to call into.
///
/// Every backend (udev, x11, winit) implements this, which lets everything in
/// `handlers/`, `grabs/` and `input.rs` stay generic over `State<BackendData>`
/// instead of being duplicated per backend.
pub trait Backend {
    fn seat_name(&self) -> String;
}

pub struct State<BackendData: Backend + 'static> {
    pub backend_data: BackendData,

    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, State<BackendData>>,
    pub running: bool,

    pub space: Space<Window>,
    pub popups: PopupManager,

    // Smithay protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    // Never read again after construction; kept alive so the xdg-output global it registers
    // stays valid for the lifetime of the compositor.
    #[allow(dead_code)]
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,

    pub seat: Seat<Self>,
}

impl<BackendData: Backend + 'static> State<BackendData> {
    pub fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        backend_data: BackendData,
    ) -> Self {
        let start_time = std::time::Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        let mut seat_state = SeatState::new();
        let seat_name = backend_data.seat_name();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, seat_name);
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop);
        Self::init_ipc_listener(&event_loop.handle(), "ipcc");

        Self {
            backend_data,

            start_time,
            socket_name,
            display_handle: dh,
            loop_handle: event_loop.handle(),
            running: true,

            space,
            popups,

            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,

            seat,
        }
    }

    fn init_wayland_listener(display: Display<Self>, event_loop: &mut EventLoop<'static, Self>) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: we don't drop the display
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    fn init_ipc_listener(loop_handle: &LoopHandle<'static, Self>, socket_name: &str) {
        let namespacename = socket_name.to_ns_name::<local_socket::GenericNamespaced>().expect("Failed to convert socket name to namespaced format");
        info!("Making IPC socket: {socket_name:?},{namespacename:?}");
        let listener = ListenerOptions::new().name(namespacename).nonblocking(ListenerNonblockingMode::Both).
                                     create_sync().expect("Failed to create local listener for comms socket");
        // The enum wrapper doesn't impl AsFd, but the platform-specific type inside it does,
        // so hand that to calloop directly and get it back in the callback.
        let listener_concrete = match listener {
            local_socket::Listener::UdSocket(l) => l,
        };

        loop_handle
            .insert_source(
                Generic::new(listener_concrete, Interest::READ, Mode::Level),
                move |_, listener, state| {
                    loop {
                         match listener.accept() {
                            Ok(s) => state.add_ipc_client(s),
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) => {
                                warn!("IPC accept failed: {e}");
                                break;
                            }
                         }
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init the IPC event source.");
    }

    fn add_ipc_client(&mut self, stream: UdsStream) {
        info!("Client attempting to connect to IPC socket");

        let mut read_buffer = Vec::new();
        let _ = self.loop_handle.insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, state| {
                // calloop only hands out &NoIoDrop<UdsStream> safely, but Read/Write are
                // implemented for &UdsStream, so read/write through a shared reference.
                let mut stream: &UdsStream = stream;
                let mut bread = BufReader::new(stream);

                loop {
                    match bread.read(&mut read_buffer) {
                        Ok(_) => {},
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            warn!("IPC read failed: {e}");
                            return Ok(PostAction::Remove);
                        }
                    }
                }

                //TODO: process message and respond via stream.write_all(&out);

                Ok(PostAction::Remove) // one command per connection
            },
        );
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }

    pub fn pointer(&self) -> PointerHandle<Self> {
        self.seat.get_pointer().unwrap()
    }

    /// Clamp a logical position so the pointer can never leave the bounding
    /// box of the (single) output mapped into the space.
    pub fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let Some(output) = self.space.outputs().next() else {
            return pos;
        };
        let geo = self.space.output_geometry(output).unwrap();
        let x = pos.x.clamp(geo.loc.x as f64, (geo.loc.x + geo.size.w) as f64);
        let y = pos.y.clamp(geo.loc.y as f64, (geo.loc.y + geo.size.h) as f64);
        (x, y).into()
    }
}

/// Data associated with a wayland client that connects to xrcomp.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
