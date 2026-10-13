use std::{env, io::Write};

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

use serde_json;

use xrcomp::comms::{Command, CommandResponse};

fn main() {
    let args: Vec<String> = env::args().collect();

    match args.len() {
        2 => {
            let command: Option<Command>;
            match args[1].to_lowercase().as_str() {

                "exit" => {
                    println!("Sending exit command to compositor...");
                    command = Some(Command::Exit);
                }
                "getwaylandsocket" => {
                    println!("Requesting Wayland socket from compositor...");
                    command = Some(Command::GetWaylandSocket);
                }
                _ => { 
                    command = None;
                }
            }

            match command {

                Some(cmd) => {
                    let cmdstring =  serde_json::to_string(&cmd).unwrap();
                    println!("Serialized command: {}", cmdstring);


                    let socket_printname = "ipcc";
                    let socket_name = socket_printname.to_ns_name::<GenericNamespaced>().expect("Getting IPC name failed");
                    let mut conn = Stream::connect(socket_name).expect("could not connect to IPC socket");

                    conn.write_all(cmdstring.as_bytes()).expect("failed to write command");

                    let resp: CommandResponse = match serde_json::from_reader(conn) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("Failed to parse command response: {e:?}");
                            return
                        } 
                    };

                    if resp.success {
                        println!("Command executed successfully.");
                        if resp.result.is_some() {
                            println!("Result: {}", resp.result.unwrap());
                        }
                    } else {
                        eprintln!("Command failed.");
                    }
                }
                None => print_command_help(),
            }
        }
        _ => print_usage(),
    }
}

fn print_usage() {
    println!("usage: xrcompctl <command>");
}

fn print_command_help() {
    println!("Unrecognized command. Available commands:");
    println!("  exit              - Exit the compositor");
    println!("  getwaylandsocket  - Get the Wayland socket from the compositor");
}