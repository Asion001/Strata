use anyhow::Result;
use strata_server::{AppState, Config, auth, background, routes};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
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
