use commission::{auth, http, service::orders, AppState, MIGRATOR};
use sqlx::postgres::PgPoolOptions;
use std::{env, error::Error, time::Duration};
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("commission=info,tower_http=info")),
        )
        .init();
    let command = env::args().nth(1).unwrap_or_else(|| "serve".into());
    if command == "generate-token" {
        println!("{}", auth::generate_secret());
        return Ok(());
    }
    if !["serve", "migrate", "bootstrap"].contains(&command.as_str()) {
        return Err("用法：commissiond [serve|migrate|bootstrap|generate-token]".into());
    }
    let database_url = env::var("DATABASE_URL").map_err(|_| "必须设置 DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    match command.as_str() {
        "migrate" => {
            MIGRATOR.run(&pool).await?;
            println!("数据库迁移完成");
            return Ok(());
        }
        "bootstrap" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&auth::bootstrap(&pool).await?)?
            );
            return Ok(());
        }
        _ => {}
    }
    // Fail closed if schema has not been migrated. Server does not need DDL rights.
    let applied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success AND version=1")
            .fetch_one(&pool)
            .await?;
    if applied != 1 {
        return Err("请先运行 commissiond migrate".into());
    }
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let release_enabled = env::var("RELEASE_WORKER").unwrap_or_else(|_| "true".into()) == "true";
    let worker_pool = pool.clone();
    let mut worker_shutdown = shutdown_rx.clone();
    let worker = tokio::spawn(async move {
        if !release_enabled {
            return;
        }
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = worker_shutdown.changed() => break,
                _ = interval.tick() => {
                    for _ in 0..32 {
                        if *worker_shutdown.borrow() { return; }
                        match orders::release_one_due(&worker_pool).await {
                            Ok(true) => {},
                            Ok(false) => break,
                            Err(error) => { tracing::error!(%error,"automatic release failed"); break; }
                        }
                    }
                }
            }
        }
    });
    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(address=%bind, release_worker=release_enabled, "commission server listening");
    let app = http::router(AppState { pool: pool.clone() });
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        })
        .await?;
    worker.await?;
    pool.close().await;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
