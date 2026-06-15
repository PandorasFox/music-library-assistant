//! mm-tui: Terminal UI client for Music Magic.
//!
//! Connects to a running mm server over its Unix domain socket.

use std::path::PathBuf;

use mm_meta::wire::{find_socket_path, SYSTEM_SOCKET_PATH};

fn main() {
    let socket_path = parse_socket_path();

    let stream = match std::os::unix::net::UnixStream::connect(&socket_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to connect to mm server at {}: {}", socket_path.display(), e);
            eprintln!("is the mm server running?");
            std::process::exit(1);
        }
    };

    // Clear terminal
    print!("\x1B[2J\x1B[1;1H");

    if let Err(e) = mm_tui::run_tui(stream) {
        eprintln!("TUI error: {:?}", e);
        std::process::exit(1);
    }
}

fn parse_socket_path() -> PathBuf {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--socket" || arg == "-s" {
            return match args.next() {
                Some(path) => PathBuf::from(path),
                None => {
                    eprintln!("error: --socket requires a path argument");
                    std::process::exit(1);
                }
            };
        }
        if arg == "--help" || arg == "-h" {
            eprintln!("Usage: mm-tui [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  -s, --socket <PATH>  Unix socket to connect to");
            eprintln!("                       (default: $XDG_RUNTIME_DIR/mm.sock or {SYSTEM_SOCKET_PATH})");
            eprintln!("  -h, --help           Show this help");
            std::process::exit(0);
        }
        eprintln!("error: unknown argument: {}", arg.to_string_lossy());
        eprintln!("try: mm-tui --help");
        std::process::exit(1);
    }
    find_socket_path().unwrap_or_else(|| {
        eprintln!("error: no mm server socket found");
        eprintln!("checked: $XDG_RUNTIME_DIR/mm.sock and {SYSTEM_SOCKET_PATH}");
        eprintln!("is the mm server running? (use --socket to specify path)");
        std::process::exit(1);
    })
}
