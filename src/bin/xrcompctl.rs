use std::io::Write;

use clap::Parser;

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

use serde_json;

use xrcomp::comms::{Command, CommandResponse, get_default_socket_name};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// optional local socket name (tries the default if not given)
    socket_name: Option<String>,

    /// Increased verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbosity: u8,

    #[command(subcommand)]
    command: Command,
}


fn main() {
    let cli = Cli::parse();

    let socket_printname = match cli.socket_name {
        Some(n) => n,
        None => get_default_socket_name()
    };

    let cmdstring =  serde_json::to_string(&cli.command).unwrap();
    if cli.verbosity > 0 { println!("Serialized command to send: {}", cmdstring); }

    let socket_name = socket_printname.to_ns_name::<GenericNamespaced>().expect("Getting IPC name failed");
    let mut conn = Stream::connect(socket_name).expect("could not connect to IPC socket");
    conn.write_all(cmdstring.as_bytes()).expect("failed to write command");

    if cli.verbosity > 0 { println!("Sent command"); }

    let resp: CommandResponse = match serde_json::from_reader(conn) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to parse command response: {e:?}");
            return
        } 
    };
    if resp.success {
        if cli.verbosity > 0 { println!("Command executed successfully."); }
        match resp.result {
            Some(r) => { 
                if cli.verbosity > 0 { println!("Command has a response:"); }
                println!("{r}");
            },
            None => {
                if cli.verbosity > 0 { println!("Command has no response."); }
            }
        } 
    } else {
        eprintln!("Command failed.");
    }
}
