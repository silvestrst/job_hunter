use crate::server::{router, AppState};
use anyhow::{Context, Result};
use job_hunter_core::config::Config;
use job_hunter_core::db::Db;
use job_hunter_core::paths::Paths;
use job_hunter_core::token;
use std::sync::{Arc, Mutex};

pub fn run(port_override: Option<u16>) -> Result<()> {
    let paths = Paths::resolve()?;
    if !paths.config_file.exists() {
        anyhow::bail!(
            "no config at {} - run `job-hunter config init` first",
            paths.config_file.display()
        );
    }
    let config = Config::load(&paths.config_file)?;
    let port = port_override.unwrap_or(config.server.port);
    let db = Db::open(&paths.db_file)?;
    let api_token = token::load_or_create(&paths.api_token_file)?;

    let state = AppState {
        db: Arc::new(Mutex::new(db)),
        token: api_token.clone(),
    };

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        // Loopback only, unconditionally: this server is for the local browser.
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("cannot bind {addr} - is another serve running?"))?;
        println!("job-hunter server listening on http://{addr}");
        println!("API token (paste into the extension popup once):\n  {api_token}");
        println!("Press Ctrl-C to stop.");
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
                println!("\nshutting down");
            })
            .await
            .context("server error")
    })
}
