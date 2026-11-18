use anyhow::Result;
use strata_server::{AppState, Config, auth, background, routes};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Small offline helpers (used by scripts/smoke.sh):
    //   strata chunk <file>  -> "offset size blake3" per FastCDC chunk
    //   strata hash <file>   -> BLAKE3 of the whole file
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("chunk") => return chunk_file(args.get(2).map(String::as_str)),
        Some("hash") => {
            let path = args
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("usage: strata hash <file>"))?;
            let mut h = blake3::Hasher::new();
            h.update_reader(std::fs::File::open(path)?)?;
            println!("{}", h.finalize().to_hex());
            return Ok(());
        }
        _ => {}
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn,tower_http=warn".into()),
        )
        .init();
    let cfg = Config::from_env()?;
    let bind = cfg.bind.clone();
    let st = AppState::new(cfg).await?;
    auth::bootstrap_admin(&st).await?;
    // Before serving: overlap checks must never see stale locations.
    strata_server::connections::backfill_locations(&st).await;
    background::spawn(st.clone());
    strata_server::journal::spawn_listener(st.clone());
    let app = routes::router(st);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("strata listening on {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! { _ = ctrl_c => {}, _ = term.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}

fn chunk_file(path: Option<&str>) -> Result<()> {
    let data = std::fs::read(path.ok_or_else(|| anyhow::anyhow!("usage: strata chunk <file>"))?)?;
    let mut off = 0;
    for len in strata_storage::chunker::chunk_lengths(&data) {
        println!(
            "{off} {len} {}",
            blake3::hash(&data[off..off + len]).to_hex()
        );
        off += len;
    }
    Ok(())
}
