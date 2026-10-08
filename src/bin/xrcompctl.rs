use std::env;
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
                    // TODO: implement IPC to send the command to the compositor and receive a response
                    let resp = CommandResponse {
                        command: cmd,
                        success: true,
                        result: Some("dummy_response".to_string()),
                    };

                    if resp.success {
                        println!("Command executed successfully.");
                        if resp.result.is_some() {
                            println!("Result: {}", resp.result.unwrap());
                        }
                    } else {
                        println!("Command failed.");
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
    println!("  exit                - Exit the compositor");
    println!("  getwaylandsocket  - Get the Wayland socket from the compositor");
}