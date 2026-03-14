//! mm-tui: Terminal UI client for Music Magic.
//!
//! Connects to a running mm server over its Unix domain socket.

use mm_meta::wire::default_socket_path;

fn main() {
    let socket_path = default_socket_path().unwrap_or_else(|| {
        eprintln!("error: XDG_RUNTIME_DIR not set");
        std::process::exit(1);
    });

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
