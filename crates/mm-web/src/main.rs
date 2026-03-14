use std::path::PathBuf;

use anyhow::Result;

use mm_web::{router, AppState};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let mut socket_path = None;
    let mut listen_addr = "0.0.0.0:3313".to_string();
    let mut static_dir = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => {
                i += 1;
                socket_path = Some(
                    args.get(i)
                        .expect("--socket requires a path")
                        .clone(),
                );
            }
            "--listen" => {
                i += 1;
                listen_addr = args.get(i)
                    .expect("--listen requires an address")
                    .clone();
            }
            "--static-dir" => {
                i += 1;
                static_dir = Some(
                    args.get(i)
                        .expect("--static-dir requires a path")
                        .clone(),
                );
            }
            other => {
                eprintln!("Unknown argument: {other}");
                eprintln!("Usage: mm-web [--socket /path/to/mm.sock] [--listen 0.0.0.0:3313] [--static-dir ./static]");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let socket_path = PathBuf::from(socket_path.unwrap_or_else(|| {
        let runtime_dir =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        format!("{runtime_dir}/mm.sock")
    }));

    // Default static dir: adjacent to the binary's crate source.
    let static_dir = PathBuf::from(static_dir.unwrap_or_else(|| {
        "crates/mm-web/static".into()
    }));

    if !static_dir.join("index.html").exists() {
        eprintln!(
            "warning: {}/index.html not found — UI will not load",
            static_dir.display()
        );
    }

    let state = AppState::new(socket_path.clone(), static_dir).await?;
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    eprintln!(
        "mm-web listening on {listen_addr} (witch: {})",
        socket_path.display()
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
            eprintln!("\nshutting down");
        })
        .await?;

    Ok(())
}
