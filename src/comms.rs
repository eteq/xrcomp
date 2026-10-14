// Data structures, etc for IPC and control of the compositor from an external process

use clap::Subcommand;
use serde::{Serialize, Deserialize};

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

#[derive(Serialize, Deserialize, Debug, Subcommand)]
pub enum Command {
    /// Have xrcomp stop gracefully
    Exit,
    /// Return the socket name of the wayland socket for this xrcomp
    GetWaylandSocket,
    /// Run the provided command as a child of xrcomp
    RunCommand {cmd: String},
}


#[derive(Serialize, Deserialize, Debug)]
pub struct CommandResponse {
    pub command: Command,
    pub success: bool,
    pub result: Option<String>,
}


const DEFAULT_LOCAL_SOCKET_BASE: &str = "xrcomp-ipc-socket-";

pub fn get_default_socket_name() -> String {
    let mut s = String::from(DEFAULT_LOCAL_SOCKET_BASE);
    s.push('1');
    s
}

pub fn get_first_free_default_socket_name() -> String {
    let mut count = 0;
    
    let mut toret = None;
    while toret.is_none() {
        let mut totry = String::from(DEFAULT_LOCAL_SOCKET_BASE);
        count += 1;
        totry.push_str(&count.to_string());

        let socket_name = totry.clone().to_ns_name::<GenericNamespaced>().expect("Getting IPC name failed");
        toret = match Stream::connect(socket_name) {
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Some(totry),
            Err(e) => {
                panic!("Unrecognized error while trying to get socket: {e:?}");
            }
        };
    }
    toret.unwrap()
}