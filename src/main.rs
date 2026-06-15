//! Music Magic (MM)
//!
//! A toolkit of precise, limited tools leveraging a common central database.
//! Main operational areas: scan, report, repair, deploy.
//!
//! Organized around librarian workflow cycles:
//! - Insight & Health: Understanding corpus state
//! - Intake: Bringing external material into corpus
//! - Organization: Corpus-mutating operations
//! - Deployment: Publishing to browsable libraries
//! - Operations: Low-level maintenance

// ============================================================================
// Allocator
// ============================================================================

// Use jemalloc instead of glibc malloc. Jemalloc returns freed arenas back to
// the OS more aggressively, which matters a lot for MM's burst-then-idle
// computation pattern (startup computation burst allocates ~3GB, then frees it
// all — glibc malloc would hold those arenas indefinitely).
//
// Configuration: background_thread purges dirty pages on a 1s decay timer.
// Without background_thread, decay only happens on the next allocation.
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc build-time configuration string (overrides the weak _rjem_malloc_conf symbol).
/// tikv-jemallocator uses the _rjem_ prefix internally.
/// background_thread:true — enables the purge thread
/// dirty_decay_ms:1000    — dirty (freed) pages returned to OS after ~1s idle
/// muzzy_decay_ms:5000    — muzzy (decommitted) pages cleaned up after ~5s
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static _rjem_malloc_conf: &[u8] =
    b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:5000\0";

// ============================================================================
// Version Information
// ============================================================================

pub use mm_meta::{MM_TITLE, MM_VERSION};

mod auth;
mod config;
mod corpus;
mod db;
mod external;
mod logging;
mod meta;
mod witch;
pub mod zones;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// Music Magic server
#[derive(Parser)]
#[command(name = "mm", about = "Music Magic server daemon")]
struct Cli {
    /// Initialize database with a user and exit (for declarative NixOS setup)
    #[arg(long)]
    init_user: Option<String>,

    /// Path to file containing password for --init-user
    #[arg(long, requires = "init_user")]
    password_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle --init-user mode: create config, DB, user, then exit
    if let Some(username) = cli.init_user {
        return init_user_and_exit(&username, cli.password_file);
    }

    // Initialize log channel FIRST (before any logging happens)
    let log_rx = logging::init_log_channel();

    // Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Witch owns the main thread. Clients connect over Unix socket.
    witch::Witch::run(Some(log_rx)).await;

    Ok(())
}

/// Initialize database and create first user, then exit.
/// Used for declarative NixOS setup where secrets are provided via files.
fn init_user_and_exit(username: &str, password_file: Option<PathBuf>) -> Result<()> {
    use mm_meta::auth::FirstTimeSetupToken;

    let password_file = password_file
        .ok_or_else(|| anyhow::anyhow!("--password-file is required with --init-user"))?;

    let password = std::fs::read_to_string(&password_file)
        .map_err(|e| anyhow::anyhow!("Failed to read password file: {}", e))?
        .trim()
        .to_string();

    if password.is_empty() {
        anyhow::bail!("Password file is empty");
    }

    // Load config (must exist)
    let cfg = config::load_config()
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    // Check if DB already exists
    let db_path = config::get_db_path()?;
    if db_path.exists() {
        eprintln!("Database already exists at {}", db_path.display());
        eprintln!("User initialization skipped (already set up)");
        return Ok(());
    }

    // Create directories
    std::fs::create_dir_all(&cfg.storage_root)?;
    std::fs::create_dir_all(cfg.libraries_dir())?;
    std::fs::create_dir_all(cfg.stash_dir())?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Create database
    let token = FirstTimeSetupToken::new();
    let db = db::create_database(&db_path, &token)?;

    // Hash password and create user
    let password_hash = auth::hash_password(&password)?;
    db.create_user(username, &password_hash)?;

    eprintln!("Initialized database at {}", db_path.display());
    eprintln!("Created user '{}'", username);

    Ok(())
}
