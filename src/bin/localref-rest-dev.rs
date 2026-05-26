//! Manual development server for the Localref user-facing REST API.
//!
//! This binary is intentionally dev-only. The production application entry is
//! the root `localref` binary; this server exists so REST API behavior can be
//! exercised in isolation during development.

use localref_core::config::LocalrefConfig;
use localref_core::rest::serve;
use localref_core::storage::StorageDb;

/// Start the Localref REST development server.
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config =
        LocalrefConfig::load().expect("failed to load Localref configuration");
    let library_root = config.library_root();
    let addr = config.rest_addr();

    let storage = StorageDb::open(library_root)
        .expect("failed to open Localref query database");

    println!("localref REST dev server listening on http://{addr}");
    println!("config: {}", config.source_path().display());
    println!("library: {}", library_root.display());
    println!("POST http://{addr}/api/daemon/scan");
    println!("GET  http://{addr}/api/items");
    println!("GET  http://{addr}/api/search?q=term");

    serve(addr, storage).await
}
