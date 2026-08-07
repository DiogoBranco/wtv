use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::sync::RwLock;
use tower_http::services::{ServeDir, ServeFile};
use wtv::core;

struct App {
    config: RwLock<core::Config>,
    config_path: PathBuf,
}

fn status(error: &'static str) -> StatusCode {
    match error {
        "not found" => StatusCode::NOT_FOUND,
        "forbidden" => StatusCode::FORBIDDEN,
        "invalid base" => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn validated(app: &App, path: &PathBuf) -> Result<PathBuf, StatusCode> {
    core::validate_worktree(&*app.config.read().await, path).map_err(status)
}

async fn worktrees(State(app): State<Arc<App>>) -> Json<Vec<core::RepoWorktrees>> {
    Json(core::repo_worktrees(&*app.config.read().await))
}

#[derive(Deserialize)]
struct WorktreeQuery { worktree: PathBuf }

async fn files(State(app): State<Arc<App>>, Query(q): Query<WorktreeQuery>) -> Result<Json<Vec<String>>, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    core::files(&wt).map(Json).map_err(status)
}

#[derive(Deserialize)]
struct FileQuery { worktree: PathBuf, path: String }

#[derive(Serialize)]
struct FileContent { content: Option<String> }

async fn file(State(app): State<Arc<App>>, Query(q): Query<FileQuery>) -> Result<Json<FileContent>, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    core::file_content(&wt, &q.path).map(|content| Json(FileContent { content })).map_err(status)
}

#[derive(Serialize)]
struct Branches { default: Option<String>, branches: Vec<String> }

async fn branches(State(app): State<Arc<App>>, Query(q): Query<WorktreeQuery>) -> Result<Json<Branches>, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    let refs = core::branch_refs(&wt);
    Ok(Json(Branches { default: core::default_branch(&wt, &refs), branches: refs }))
}

#[derive(Deserialize)]
struct DiffQuery { worktree: PathBuf, base: String }

async fn changed(State(app): State<Arc<App>>, Query(q): Query<DiffQuery>) -> Result<Json<Vec<core::ChangedFile>>, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    core::changed_files(&wt, &q.base).map(Json).map_err(status)
}

#[derive(Deserialize)]
struct DiffFileQuery { worktree: PathBuf, base: String, path: String }

async fn diff_file(State(app): State<Arc<App>>, Query(q): Query<DiffFileQuery>) -> Result<Json<core::DiffContent>, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    core::diff_content(&wt, &q.base, &q.path).map(Json).map_err(status)
}

async fn watch(State(app): State<Arc<App>>, Query(q): Query<WorktreeQuery>, upgrade: WebSocketUpgrade) -> Result<Response, StatusCode> {
    let wt = validated(&app, &q.worktree).await?;
    Ok(upgrade.on_upgrade(move |socket| watch_socket(socket, wt)))
}

async fn watch_socket(mut socket: WebSocket, worktree: PathBuf) {
    let (tx, rx) = mpsc::channel();
    let Ok(_watcher) = core::watch(&worktree, tx) else { return };
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                _ => {}
            },
            _ = tick.tick() => if rx.try_recv().is_ok() {
                tokio::time::sleep(Duration::from_millis(150)).await;
                while rx.try_recv().is_ok() {}
                if socket.send(Message::Text("change".into())).await.is_err() { break; }
            }
        }
    }
}

#[derive(Deserialize)]
struct AskBody {
    worktree: PathBuf,
    agent: String,
    file: String,
    lines: Option<String>,
    base: Option<String>,
    question: String,
}

#[derive(Serialize)]
struct ErrorBody { error: String }

async fn ask(State(app): State<Arc<App>>, Json(body): Json<AskBody>) -> Result<StatusCode, (StatusCode, Json<ErrorBody>)> {
    let wt = validated(&app, &body.worktree).await.map_err(|s| (s, Json(ErrorBody { error: "invalid worktree".into() })))?;
    core::inject(&wt, &body.agent, &body.file, body.lines.as_deref(), body.base.as_deref(), &body.question)
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|error| {
            let code = if error.starts_with("no ") { StatusCode::NOT_FOUND } else if error == "injection failed" { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::BAD_REQUEST };
            (code, Json(ErrorBody { error }))
        })
}

#[derive(Serialize, Deserialize)]
struct AccentBody { accent: String }

async fn get_config(State(app): State<Arc<App>>) -> Json<AccentBody> {
    Json(AccentBody { accent: app.config.read().await.accent.clone() })
}

async fn set_config(State(app): State<Arc<App>>, Json(body): Json<AccentBody>) -> StatusCode {
    let mut config = app.config.write().await;
    config.accent = body.accent;
    match toml::to_string_pretty(&*config) {
        Ok(value) if std::fs::write(&app.config_path, &value).is_ok() => StatusCode::NO_CONTENT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[tokio::main]
async fn main() {
    let path = core::config_path(std::env::args()).unwrap_or_else(|e| panic!("{e}"));
    let config = core::load_config(&path).unwrap_or_else(|e| panic!("{e}"));
    let app = Arc::new(App { config: RwLock::new(config), config_path: path });
    let router = Router::new()
        .route("/api/worktrees", get(worktrees))
        .route("/api/files", get(files))
        .route("/api/file", get(file))
        .route("/api/branches", get(branches))
        .route("/api/changed", get(changed))
        .route("/api/diff-file", get(diff_file))
        .route("/api/watch", get(watch))
        .route("/api/ask", post(ask))
        .route("/api/config", get(get_config).post(set_config))
        .fallback_service(ServeDir::new("web/dist").fallback(ServeFile::new("web/dist/index.html")))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7345").await.unwrap();
    println!("wtv on http://127.0.0.1:7345");
    axum::serve(listener, router).await.unwrap();
}
