mod grabs;
mod handlers;
mod input;
mod ipc;
mod render;
mod scene;
mod state;

#[cfg(feature = "udev")]
mod udev;
#[cfg(feature = "winit")]
mod winit;
#[cfg(feature = "x11")]
mod x11;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    /// IPC socket name to use
    ipc_socket_name: Option<String>,

    #[command(subcommand)]
    backend: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[cfg(feature = "udev")]
    /// Run on a raw tty using udev/DRM/libinput (Default, requires a seat).
    Udev,
    #[cfg(feature = "winit")]
    /// Run nested in an existing X11 session. Intended for development.
    Winit,
    #[cfg(feature = "x11")]
    /// Run nested in an existing Wayland (or x11) session using winit. Intended for development.
    X11,
}

fn main() {
    init_logging();

    let cli = Cli::parse();

    let backend = match cli.backend {
        Some(b)=> b,
        None => Commands::Udev
    };

    match backend {
        #[cfg(feature = "udev")]
        Commands::Udev => {
            tracing::info!("Starting xrcomp with the udev backend");
            udev::run_udev(cli.ipc_socket_name);
        },
        #[cfg(feature = "x11")]
        Commands::X11 => {
            tracing::info!("Starting xrcomp with the x11 backend");
            x11::run_x11(cli.ipc_socket_name);
        },
        #[cfg(feature = "winit")]
        Commands::Winit => {
            tracing::info!("Starting xrcomp with the winit backend");
            winit::run_winit(cli.ipc_socket_name);
        }
    }
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}
