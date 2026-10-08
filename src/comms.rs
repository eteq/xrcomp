// Data structures, etc for IPC and control of the compositor from an external process

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

//TODO: implement generic IPC mechanism using interprocess, hook into backends