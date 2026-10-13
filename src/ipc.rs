// Compositor-side IPC: listens on the control socket and acts on commands from xrcompctl.

use interprocess::local_socket;
use interprocess::local_socket::{prelude::*, ListenerNonblockingMode, ListenerOptions};
use interprocess::os::unix::uds_local_socket::Stream as UdsStream;

use std::io::{self, Read};

use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};

use tracing::{warn, info};

use xrcomp::comms::{Command, CommandResponse};

use crate::state::{Backend, State};

pub(crate) fn init_ipc_listener<B: Backend + 'static>(loop_handle: &LoopHandle<'static, State<B>>, socket_name: &str) {
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
                        Ok(s) => add_ipc_client(state, s),
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

fn add_ipc_client<B: Backend + 'static>(state: &mut State<B>, stream: UdsStream) {
    info!("Client attempting to connect to IPC socket");

    let mut read_buffer = Vec::new(); //TODO: FIX BUG
    let _ = state.loop_handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, mut state| {
            // calloop only hands out &NoIoDrop<UdsStream> safely, but Read/Write are
            // implemented for &UdsStream, so read/write through a shared reference.
            let mut stream: &UdsStream = stream;

            match stream.read_to_end(&mut read_buffer) {
                Ok(_) => {},
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {},
                Err(e) => {
                    warn!("IPC read failed: {e}");
                    return Ok(PostAction::Remove);
                }
            }

            match process_message(&read_buffer, &mut state) {
                Ok(response) => {
                    match serde_json::to_writer(stream, &response) {
                        Ok(_) => {},
                        Err(e) =>  warn!("IPC response write failed: {e}")
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                Err(e) => {
                    warn!("IPC message processing failed: {e}");
                }
            }

            Ok(PostAction::Remove) // one command per connection
        },
    );
}

pub fn process_message<B: Backend + 'static>(message_bytes: &[u8], state: &mut State<B>) -> Result<CommandResponse, std::io::Error> {

        //let command = Command::deserialize(deserializer)
        let command: Command = match serde_json::from_slice(message_bytes) {
            Ok(cmd) => cmd,
            Err(e) if e.is_eof() => return Err(io::Error::new(io::ErrorKind::WouldBlock, "Incomplete JSON object")),
            Err(e) => {
                warn!("Failed to deserialize IPC message: {e}");
                return Err(io::Error::new(io::ErrorKind::InvalidData, 
                    "Failed to deserialize IPC message"));
            }
        };

        let response = match command {
            Command::Exit => {
                info!("IPC command: Exit");
                state.running = false;
                CommandResponse {
                    command,
                    success: true,
                    result: None,
                }
            }
            Command::GetWaylandSocket => {
                info!("IPC command: GetWaylandSocket");
                let socket_name = state.socket_name.clone();
                CommandResponse {
                    command,
                    success: true,
                    result: Some(socket_name.to_string_lossy().to_string()),
                }
            }
        };        

        Ok(response)
}
