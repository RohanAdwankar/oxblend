use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tokio::net::TcpListener;

use crate::bridge::run_blender_export;
use crate::parser::parse_scene;
use crate::scene::OutputFormat;

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>oxblend Viewer</title>
  <script type="module" src="https://unpkg.com/@google/model-viewer/dist/model-viewer.min.js"></script>
  <style>
    :root {
      --bg: #f3efe6;
      --panel: #fffaf1;
      --line: #d8cfbe;
      --text: #1f2421;
      --muted: #6d746f;
      --accent: #0d6b5d;
      --error: #9d1c1c;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      font-family: ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      color: var(--text);
      background:
        radial-gradient(circle at top left, #fffdf8, transparent 35%),
        linear-gradient(135deg, #ece4d6, var(--bg));
      height: 100vh;
      overflow: hidden;
    }
    .layout {
      display: grid;
      grid-template-columns: 1.15fr 0.85fr;
      height: 100vh;
      gap: 12px;
      padding: 12px;
    }
    .panel {
      border: 1px solid var(--line);
      background: rgba(255, 250, 241, 0.88);
      backdrop-filter: blur(14px);
      border-radius: 18px;
      overflow: hidden;
      box-shadow: 0 18px 50px rgba(60, 50, 34, 0.08);
    }
    .panel-header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      padding: 14px 16px;
      border-bottom: 1px solid var(--line);
      font-size: 14px;
      letter-spacing: 0.01em;
      color: var(--muted);
      background: rgba(255, 255, 255, 0.35);
    }
    .header-actions {
      display: flex;
      align-items: center;
      gap: 12px;
    }
    .download-link {
      border: 1px solid var(--line);
      border-radius: 999px;
      padding: 7px 12px;
      text-decoration: none;
      color: var(--text);
      background: rgba(255,255,255,0.72);
      transition: background 140ms ease, transform 140ms ease;
    }
    .download-link:hover {
      background: #ffffff;
      transform: translateY(-1px);
    }
    .viewer-wrap {
      height: calc(100vh - 84px);
      display: flex;
      flex-direction: column;
    }
    model-viewer {
      width: 100%;
      flex: 1;
      background:
        radial-gradient(circle at 25% 20%, rgba(255,255,255,0.92), rgba(255,255,255,0) 35%),
        linear-gradient(180deg, #d5e0d8 0%, #becdbf 100%);
    }
    .status-bar {
      border-top: 1px solid var(--line);
      padding: 10px 14px;
      font-size: 13px;
      color: var(--muted);
      background: rgba(255,255,255,0.5);
      min-height: 42px;
    }
    textarea {
      width: 100%;
      height: calc(100vh - 84px);
      border: 0;
      outline: none;
      resize: none;
      background: transparent;
      color: var(--text);
      font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
      padding: 16px;
      tab-size: 2;
    }
    .dot {
      width: 10px;
      height: 10px;
      border-radius: 999px;
      background: var(--muted);
      display: inline-block;
      margin-right: 8px;
    }
    .dot.ready { background: var(--accent); }
    .dot.error { background: var(--error); }
    .dot.rendering { background: #b8860b; }
    @media (max-width: 980px) {
      .layout {
        grid-template-columns: 1fr;
        grid-template-rows: 0.95fr 1.05fr;
      }
      .viewer-wrap, textarea {
        height: auto;
      }
    }
  </style>
</head>
<body>
  <div class="layout">
    <section class="panel">
      <div class="panel-header">
        <span>Preview</span>
        <div class="header-actions">
          <a id="downloadLink" class="download-link" href="/model.glb" download="preview.glb">Download 3D</a>
          <span id="renderVersion">render 0</span>
        </div>
      </div>
      <div class="viewer-wrap">
        <model-viewer id="viewer" camera-controls auto-rotate shadow-intensity="1.0" shadow-softness="0.0" exposure="1.0" environment-image="neutral"></model-viewer>
        <div class="status-bar"><span id="statusDot" class="dot"></span><span id="statusText">Starting viewer...</span></div>
      </div>
    </section>
    <section class="panel">
      <div class="panel-header">
        <span>Source</span>
        <span id="saveText">idle</span>
      </div>
      <textarea id="source" spellcheck="false"></textarea>
    </section>
  </div>
  <script>
    const viewer = document.getElementById("viewer");
    const source = document.getElementById("source");
    const statusText = document.getElementById("statusText");
    const statusDot = document.getElementById("statusDot");
    const renderVersion = document.getElementById("renderVersion");
    const saveText = document.getElementById("saveText");
    const downloadLink = document.getElementById("downloadLink");
    let lastRenderVersion = -1;
    let lastSourceVersion = -1;
    let saveTimer = null;
    let isSaving = false;
    let isDirty = false;

    async function loadSource(force = false) {
      if (!force && (document.activeElement === source || isDirty)) {
        return;
      }
      const response = await fetch("/api/source");
      const payload = await response.json();
      source.value = payload.content;
      lastSourceVersion = payload.source_version;
    }

    async function saveSource() {
      isSaving = true;
      saveText.textContent = "saving...";
      const response = await fetch("/api/source", {
        method: "POST",
        headers: { "Content-Type": "text/plain; charset=utf-8" },
        body: source.value,
      });
      isSaving = false;
      if (!response.ok) {
        saveText.textContent = "save failed";
        return;
      }
      const payload = await response.json();
      lastSourceVersion = payload.source_version;
      isDirty = false;
      saveText.textContent = "saved";
    }

    source.addEventListener("input", () => {
      isDirty = true;
      saveText.textContent = "editing...";
      clearTimeout(saveTimer);
      saveTimer = setTimeout(saveSource, 350);
    });

    async function pollState() {
      try {
        const response = await fetch("/api/state");
        const state = await response.json();
        renderVersion.textContent = `render ${state.render_version}`;
        statusText.textContent = state.message;
        statusDot.className = `dot ${state.status}`;

        if (state.render_version !== lastRenderVersion && state.status !== "error") {
          const modelUrl = `/model.glb?v=${state.render_version}`;
          viewer.src = modelUrl;
          downloadLink.href = modelUrl;
          lastRenderVersion = state.render_version;
        }

        if (state.source_version !== lastSourceVersion && !isSaving) {
          await loadSource();
        }
      } catch (error) {
        statusText.textContent = `Viewer connection error: ${error}`;
        statusDot.className = "dot error";
      }
    }

    loadSource(true).then(pollState);
    setInterval(pollState, 700);
  </script>
</body>
</html>
"#;

#[derive(Clone)]
struct AppState {
    source_path: PathBuf,
    model_path: PathBuf,
    state: Arc<Mutex<ViewerState>>,
    trigger: mpsc::Sender<()>,
    source_cache: Arc<Mutex<String>>,
}

#[derive(Debug, Clone, Serialize)]
struct ViewerState {
    render_version: u64,
    source_version: u64,
    status: &'static str,
    message: String,
}

impl ViewerState {
    fn new() -> Self {
        Self {
            render_version: 0,
            source_version: 0,
            status: "rendering",
            message: "Preparing initial preview...".to_string(),
        }
    }
}

#[derive(Serialize)]
struct SourcePayload {
    content: String,
    source_version: u64,
}

pub async fn run_viewer(input: PathBuf, blender_bin: Option<PathBuf>) -> Result<()> {
    let source = fs::read_to_string(&input)
        .with_context(|| format!("failed to read input file {}", input.display()))?;

    let temp_dir = make_temp_dir()?;
    let model_path = temp_dir.join("preview.glb");
    let source_cache = Arc::new(Mutex::new(source));
    let state = Arc::new(Mutex::new(ViewerState::new()));
    let (tx, rx) = mpsc::channel();

    start_render_worker(
        input.clone(),
        model_path.clone(),
        blender_bin.clone(),
        state.clone(),
        source_cache.clone(),
        rx,
    );
    let _watcher = start_file_watcher(input.clone(), tx.clone())?;
    let _ = tx.send(());

    let app_state = AppState {
        source_path: input.clone(),
        model_path,
        state,
        trigger: tx,
        source_cache,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/source", get(get_source).post(update_source))
        .route("/model.glb", get(get_model))
        .with_state(app_state);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let url = format!("http://{}", address);

    let open_message = match webbrowser::open(&url) {
        Ok(_) => "Opened live viewer in your browser.".to_string(),
        Err(_) => format!("Live viewer is available at {}", url),
    };
    println!("{open_message}");

    axum::serve(listener, app)
        .await
        .context("viewer server exited unexpectedly")
}

fn start_render_worker(
    source_path: PathBuf,
    model_path: PathBuf,
    blender_bin: Option<PathBuf>,
    state: Arc<Mutex<ViewerState>>,
    source_cache: Arc<Mutex<String>>,
    rx: mpsc::Receiver<()>,
) {
    thread::spawn(move || {
        while rx.recv().is_ok() {
            while rx.recv_timeout(Duration::from_millis(250)).is_ok() {}
            render_scene(
                &source_path,
                &model_path,
                blender_bin.as_deref(),
                &state,
                &source_cache,
            );
        }
    });
}

fn start_file_watcher(
    source_path: PathBuf,
    trigger: mpsc::Sender<()>,
) -> Result<RecommendedWatcher> {
    let watched_path = source_path.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let is_relevant_kind = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            );
            let touches_source = event.paths.iter().any(|path| path == &watched_path);
            if is_relevant_kind && touches_source {
                let _ = trigger.send(());
            }
        }
    })?;
    watcher.watch(&source_path, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

fn render_scene(
    source_path: &Path,
    model_path: &Path,
    blender_bin: Option<&Path>,
    state: &Arc<Mutex<ViewerState>>,
    source_cache: &Arc<Mutex<String>>,
) {
    {
        let mut state = state.lock().unwrap();
        state.status = "rendering";
        state.message = format!("Rendering {}...", source_path.display());
    }

    let source = match fs::read_to_string(source_path) {
        Ok(source) => source,
        Err(error) => {
            let mut state = state.lock().unwrap();
            state.status = "error";
            state.message = format!("Failed to read source: {error}");
            return;
        }
    };

    {
        let mut cached = source_cache.lock().unwrap();
        if *cached != source {
            *cached = source.clone();
            let mut state = state.lock().unwrap();
            state.source_version += 1;
        }
    }

    let outcome = (|| -> Result<()> {
        let mut scene = parse_scene(&source)
            .with_context(|| format!("failed to parse {}", source_path.display()))?;
        scene.validate()?;
        run_blender_export(&mut scene, model_path, OutputFormat::Glb, blender_bin)?;
        Ok(())
    })();

    let mut state = state.lock().unwrap();
    match outcome {
        Ok(()) => {
            state.render_version += 1;
            state.status = "ready";
            state.message = format!("Preview synced from {}", source_path.display());
        }
        Err(error) => {
            state.status = "error";
            state.message = error.to_string();
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn get_state(State(state): State<AppState>) -> Json<ViewerState> {
    Json(state.state.lock().unwrap().clone())
}

async fn get_source(State(state): State<AppState>) -> Json<SourcePayload> {
    let content = state.source_cache.lock().unwrap().clone();
    let source_version = state.state.lock().unwrap().source_version;
    Json(SourcePayload {
        content,
        source_version,
    })
}

async fn update_source(State(state): State<AppState>, body: String) -> impl IntoResponse {
    match fs::write(&state.source_path, &body) {
        Ok(()) => {
            *state.source_cache.lock().unwrap() = body;
            {
                let mut viewer_state = state.state.lock().unwrap();
                viewer_state.source_version += 1;
                viewer_state.status = "rendering";
                viewer_state.message =
                    format!("Saved {}, updating preview...", state.source_path.display());
            }
            let _ = state.trigger.send(());
            (
                StatusCode::OK,
                Json(SourcePayload {
                    content: state.source_cache.lock().unwrap().clone(),
                    source_version: state.state.lock().unwrap().source_version,
                }),
            )
                .into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write source file: {error}"),
        )
            .into_response(),
    }
}

async fn get_model(State(state): State<AppState>) -> Response {
    match fs::read(&state.model_path) {
        Ok(bytes) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("model/gltf-binary"),
            )],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "preview not available yet").into_response(),
    }
}

fn make_temp_dir() -> Result<PathBuf> {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("oxblend-view-{}", suffix));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}
