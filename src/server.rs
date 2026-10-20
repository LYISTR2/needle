use crate::shared::{self, Report};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub origin: String,
    pub database: PathBuf,
    pub socket: PathBuf,
    pub viewer_hash: String,
    #[serde(default)]
    pub theme: BTreeMap<String, String>,
}

#[derive(Clone)]
struct App {
    db: Arc<Mutex<Connection>>,
    host: String,
    origin: String,
    viewer_hash: String,
    theme_css: String,
}

pub fn open_db(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS nodes (
            id TEXT PRIMARY KEY, name TEXT NOT NULL, token_hash TEXT NOT NULL,
            sequence INTEGER NOT NULL DEFAULT 0, received_at INTEGER,
            report TEXT
        );",
    )?;
    Ok(db)
}

pub fn initialize(path: &Path, value: &str, database: PathBuf, socket: PathBuf) -> Result<()> {
    let url = shared::origin(value)?;
    ensure!(
        database.is_absolute() && socket.is_absolute(),
        "database and socket paths must be absolute"
    );
    let viewer = shared::token()?;
    let cfg = Config {
        origin: url.origin().ascii_serialization(),
        database,
        socket,
        viewer_hash: shared::digest(&viewer),
        theme: BTreeMap::new(),
    };
    shared::create_json(path, &cfg)?;
    println!(
        "Panel config created: {}\nDashboard token (shown only now): {viewer}\nStore it in a password manager. It cannot mutate nodes.",
        path.display()
    );
    Ok(())
}

pub fn add_node(config: &Path, id: &str, name: &str, output: &Path, interface: &str) -> Result<()> {
    shared::valid_id(id)?;
    ensure!(
        !name.is_empty() && name.len() <= 80 && !name.chars().any(char::is_control),
        "invalid node name"
    );
    crate::agent::valid_interface(interface)?;
    let cfg: Config = shared::read_json(config)?;
    shared::origin(&cfg.origin)?;
    let mut db = open_db(&cfg.database)?;
    let tx = db.transaction()?;
    let count: i64 = tx.query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))?;
    ensure!(count < 256, "node limit is 256");
    let key = shared::token()?;
    tx.execute(
        "INSERT INTO nodes (id,name,token_hash) VALUES (?1,?2,?3)",
        params![id, name, shared::digest(&key)],
    )?;
    let ac = crate::agent::Config {
        endpoint: cfg.origin,
        node_id: id.into(),
        token: key,
        state_dir: PathBuf::from("/var/lib/needle-agent"),
        interfaces: vec![interface.into()],
        interval_seconds: 10,
        targets: vec![],
        ca_certificate: None,
        path_prefix: String::new(),
    };
    shared::create_json(output, &ac)?;
    if let Err(e) = tx.commit() {
        let _ = fs::remove_file(output);
        return Err(e.into());
    }
    println!(
        "Node added: {id}. Agent credential written privately to {}",
        output.display()
    );
    Ok(())
}

fn theme_css(theme: &BTreeMap<String, String>) -> Result<String> {
    let keys = ["background", "surface", "text", "muted", "accent", "line"];
    let mut css = String::from(":root,:root[data-theme=dark]{");
    for (key, value) in theme {
        ensure!(
            keys.contains(&key.as_str()),
            "unsupported theme token: {key}"
        );
        ensure!(
            value.len() == 7
                && value.starts_with('#')
                && value[1..].bytes().all(|b| b.is_ascii_hexdigit()),
            "theme values must be #RRGGBB"
        );
        css.push_str(&format!("--{key}:{value};"));
    }
    css.push('}');
    Ok(css)
}

pub async fn serve(path: &Path) -> Result<()> {
    let cfg: Config = shared::read_json(path)?;
    let url = shared::origin(&cfg.origin)?;
    ensure!(
        cfg.viewer_hash.len() == 64 && hex::decode(&cfg.viewer_hash).is_ok(),
        "invalid viewer hash"
    );
    ensure!(
        cfg.socket.is_absolute() && cfg.database.is_absolute(),
        "use absolute storage paths"
    );
    let lock_path = cfg.socket.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .map_err(|_| anyhow::anyhow!("panel already running"))?;
    if let Ok(meta) = fs::symlink_metadata(&cfg.socket) {
        ensure!(
            meta.file_type().is_socket(),
            "refusing to replace a non-socket path"
        );
        ensure!(
            UnixStream::connect(&cfg.socket).is_err(),
            "socket is already active"
        );
        fs::remove_file(&cfg.socket)?;
    }
    let app = App {
        db: Arc::new(Mutex::new(open_db(&cfg.database)?)),
        host: url[url::Position::BeforeHost..url::Position::AfterPort].to_owned(),
        origin: url.origin().ascii_serialization(),
        viewer_hash: cfg.viewer_hash,
        theme_css: theme_css(&cfg.theme)?,
    };
    let listener = tokio::net::UnixListener::bind(&cfg.socket)?;
    fs::set_permissions(&cfg.socket, fs::Permissions::from_mode(0o660))?;
    let router = Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/style.css", get(style))
        .route("/theme.css", get(theme))
        .route("/api/nodes", get(nodes))
        .route("/api/report/{id}", post(report))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(16 * 1024))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(8),
        ))
        .layer(tower::limit::ConcurrencyLimitLayer::new(32))
        .layer(middleware::from_fn_with_state(app.clone(), boundary))
        .with_state(app);
    eprintln!(
        "Needle panel listening on Unix socket {} (no TCP listener)",
        cfg.socket.display()
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("signal handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        })
        .await?;
    fs::remove_file(cfg.socket)?;
    drop(lock);
    Ok(())
}

async fn boundary(State(app): State<App>, request: Request<Body>, next: Next) -> Response {
    let h = request.headers();
    let valid_host = h.get("host").and_then(|v| v.to_str().ok()) == Some(&app.host);
    let secure = h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let valid_origin = h
        .get("origin")
        .map(|v| v.to_str().ok() == Some(&app.origin))
        .unwrap_or(true);
    let mut response = if !valid_host || !secure || !valid_origin {
        (
            StatusCode::FORBIDDEN,
            "HTTPS reverse proxy and configured origin required",
        )
            .into_response()
    } else {
        next.run(request).await
    };
    let h = response.headers_mut();
    for (key, value) in [
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
        ),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        ("cache-control", "no-store"),
        ("strict-transport-security", "max-age=31536000"),
    ] {
        h.insert(key, HeaderValue::from_static(value));
    }
    response
}

fn authorized(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    let Some(raw) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    if raw.len() != 64 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    bool::from(shared::digest(raw).as_bytes().ct_eq(expected.as_bytes()))
}

async fn index() -> impl IntoResponse {
    axum::response::Html(include_str!("../web/index.html"))
}
async fn script() -> impl IntoResponse {
    (
        [("content-type", "application/javascript; charset=utf-8")],
        include_str!("../web/app.js"),
    )
}
async fn style() -> impl IntoResponse {
    (
        [("content-type", "text/css; charset=utf-8")],
        include_str!("../web/dashboard.css"),
    )
}
async fn theme(State(app): State<App>) -> impl IntoResponse {
    ([("content-type", "text/css; charset=utf-8")], app.theme_css)
}

#[derive(Serialize)]
struct Node {
    id: String,
    name: String,
    received_at: Option<u64>,
    online: bool,
    report: Option<Report>,
}
#[derive(Serialize)]
struct Snapshot {
    server_time: u64,
    nodes: Vec<Node>,
}

async fn nodes(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Snapshot>, StatusCode> {
    if !authorized(&headers, &app.viewer_hash) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = app
        .db
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = db
        .prepare("SELECT id,name,received_at,report FROM nodes ORDER BY name,id")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = shared::now();
    let rows = stmt
        .query_map([], |r| {
            let received_at: Option<u64> = r.get(2)?;
            let report: Option<String> = r.get(3)?;
            Ok(Node {
                id: r.get(0)?,
                name: r.get(1)?,
                received_at,
                online: received_at.is_some_and(|t| now.saturating_sub(t) <= 45),
                report: report.and_then(|r| serde_json::from_str(&r).ok()),
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let nodes = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(Snapshot {
        server_time: now,
        nodes,
    }))
}

async fn report(
    State(app): State<App>,
    AxumPath(id): AxumPath<String>,
    headers: axum::http::HeaderMap,
    Json(report): Json<Report>,
) -> Result<StatusCode, StatusCode> {
    shared::valid_id(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = app
        .db
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row: Option<(String, u64, Option<u64>)> = db
        .query_row(
            "SELECT token_hash,sequence,received_at FROM nodes WHERE id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some((key, sequence, received)) = row else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    if !authorized(&headers, &key) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    report
        .validate()
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if report.sequence <= sequence {
        return Err(StatusCode::CONFLICT);
    }
    let now = shared::now();
    if received.is_some_and(|t| now.saturating_sub(t) < 2) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let payload = serde_json::to_string(&report).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    db.execute(
        "UPDATE nodes SET sequence=?1,received_at=?2,report=?3 WHERE id=?4",
        params![report.sequence, now, payload, id],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_is_color_data_not_executable_css() {
        assert!(theme_css(&BTreeMap::from([("accent".into(), "#abcdef".into())])).is_ok());
        for (key, value) in [
            ("accent", "red; background:url(https://evil)"),
            ("unknown", "#ffffff"),
        ] {
            assert!(theme_css(&BTreeMap::from([(key.into(), value.into())])).is_err());
        }
    }
    #[test]
    fn bearer_scope_and_format() {
        let token = shared::token().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        assert!(authorized(&headers, &shared::digest(&token)));
        assert!(!authorized(&headers, &shared::digest("other credential")));
        assert!(!authorized(
            &axum::http::HeaderMap::new(),
            &shared::digest(&token)
        ));
    }
}
