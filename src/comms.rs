// Data structures, etc for IPC and control of the compositor from an external process

//use crate::state::{State, Backend};

use serde::{Serialize, Deserialize};


#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    Exit,
    GetWaylandSocket
    
}


#[derive(Serialize, Deserialize, Debug)]
pub struct CommandResponse {
    pub command: Command,
    pub success: bool,
    pub result: Option<String>,
}

// pub fn process_message<B: Backend + 'static>(message_bytes: Vec<u8>, state: &mut State<B>) -> Result<CommandResponse, std::io::Error> {

//         state.running = false;

//         Ok(CommandResponse {
//             command: Command::Exit,
//             success: true,
//             result: None,
//         })
// }