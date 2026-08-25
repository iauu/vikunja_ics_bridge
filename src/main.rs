mod config;
mod ics;
mod vikunja;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use config::{Bridge, Bridges};

struct AppState {
    bridges: Bridges,
    http: reqwest::Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cred_path: PathBuf = std::env::var("CRED_PATH")
        .unwrap_or_else(|_| "cred.json".to_string())
        .into();
    let bridges = match config::load(&cred_path) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("{e}");
            std::process::exit(1);
        }
    };
    for (secret, b) in &bridges {
        tracing::info!(
            "bridge {} -> {} project {} filter {:?}",
            config::redact(secret),
            b.api_root(),
            b.project_id,
            b.filter.as_deref().unwrap_or("")
        );
    }

    let http = reqwest::Client::builder()
        .user_agent(concat!("vikunja_ics_conv/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("build http client");

    let state = Arc::new(AppState { bridges, http });

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/calendar/{secret}", get(calendar))
        .with_state(state);

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()
        .expect("LISTEN_ADDR must be host:port");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("cannot bind {addr}: {e}");
            std::process::exit(1);
        });
    tracing::info!("listening on http://{addr} — GET /calendar/<secret>.ics");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// `GET /calendar/{secret}.ics` — builds the calendar fresh on every fetch.
async fn calendar(State(state): State<Arc<AppState>>, Path(secret): Path<String>) -> Response {
    let secret = secret.strip_suffix(".ics").unwrap_or(&secret);

    let Some(bridge) = state.bridges.get(secret) else {
        tracing::warn!("unknown secret {}", config::redact(secret));
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    };

    match vikunja::fetch_tasks(&state.http, bridge).await {
        Ok(tasks) => {
            let cal = ics::render(bridge, &tasks);
            tracing::info!(
                "served {} ({} events, {} tasks without start+end skipped)",
                config::redact(secret),
                cal.event_count,
                cal.skipped
            );
            ics_response(bridge, cal.body)
        }
        Err(vikunja::FetchError::Upstream { status, message }) => {
            tracing::warn!(
                "upstream {status} for {}: {message}",
                config::redact(secret)
            );
            (
                StatusCode::BAD_GATEWAY,
                format!("vikunja rejected the request ({status}): {message}\n"),
            )
                .into_response()
        }
        Err(e @ vikunja::FetchError::Transport(_)) => {
            tracing::error!("{e} for {}", config::redact(secret));
            (StatusCode::BAD_GATEWAY, format!("{e}\n")).into_response()
        }
    }
}

fn ics_response(bridge: &Bridge, body: String) -> Response {
    let filename = format!("vikunja-{}.ics", bridge.project_id);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("inline; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    (headers, body).into_response()
}
