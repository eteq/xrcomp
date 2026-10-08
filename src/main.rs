mod grabs;
mod handlers;
mod input;
mod render;
mod scene;
mod state;

#[cfg(feature = "udev")]
mod udev;
#[cfg(feature = "winit")]
mod winit;
#[cfg(feature = "x11")]
mod x11;

fn main() {
    init_logging();

    let backend: Option<String> = std::env::args().nth(1);
    match backend.as_deref() {
        #[cfg(feature = "udev")]
        Some("udev") | None => {
            tracing::info!("Starting xrcomp with the udev backend");
            udev::run_udev();
        }
        #[cfg(feature = "x11")]
        Some("x11") => {
            tracing::info!("Starting xrcomp with the x11 backend");
            x11::run_x11();
        }
        #[cfg(feature = "winit")]
        Some("winit") => {
            tracing::info!("Starting xrcomp with the winit backend");
            winit::run_winit();
        }
        Some(other) => {
            eprintln!("Unknown backend: {other}");
            print_usage();
        }
        #[allow(unreachable_patterns)]
        _ => print_usage(),
    }
}

fn print_usage() {
    println!("USAGE: xrcomp [udev | x11 | winit]");
    println!();
    println!("  udev   Run on a raw tty using udev/DRM/libinput (default, requires a seat).");
    println!("  x11    Run nested in an existing X11 session. Intended for development.");
    println!("  winit  Run nested in an existing Wayland (or x11) session using winit. Intended for development.");
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}

