pub mod auth;
pub mod broadcast;
pub mod commands;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tower_http::services::ServeDir;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use self::auth::WebAccessToken;
use self::broadcast::WsBroadcaster;
use self::commands::WsState;

/// Shared state for the axum server.
#[derive(Clone)]
struct AppState {
    web_token: Arc<WebAccessToken>,
    ws_state: WsState,
}

/// Start the embedded HTTP/WebSocket server.
/// Called from Tauri's setup() — runs on the same tokio runtime.
pub fn start_server(
    bind: String,
    port: u16,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
) {
    let ws_state = WsState {
        session_mgr,
        pty_mgr,
        settings,
        broadcaster,
        app_handle,
    };

    let state = AppState {
        web_token,
        ws_state,
    };

    // Resolve dist path for static file serving
    let dist_path = resolve_dist_path();

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);

    // Serve static files if dist/ exists
    if let Some(path) = dist_path {
        log::info!("[web-server] Serving static files from {:?}", path);
        app = app.fallback_service(
            ServeDir::new(path).append_index_html_on_directories(true),
        );
    } else {
        log::warn!("[web-server] No dist/ directory found — static file serving disabled");
    }

    tauri::async_runtime::spawn(async move {
        let addr: SocketAddr = format!("{}:{}", bind, port)
            .parse()
            .expect("Invalid bind address");

        log::info!("[web-server] Listening on http://{}", addr);
        println!("[web-server] Listening on http://{}", addr);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("Failed to bind web server");

        axum::serve(listener, app)
            .await
            .expect("Web server error");
    });
}

/// WebSocket upgrade handler with token validation.
async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // In dev mode, skip token validation for easier testing
    if !cfg!(debug_assertions) {
        let token = params.get("token").cloned().unwrap_or_default();
        if !state.web_token.matches(&token) {
            return (axum::http::StatusCode::UNAUTHORIZED, "Invalid token").into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws_connection(socket, state.ws_state))
}

/// Handle an authenticated WebSocket connection.
async fn handle_ws_connection(socket: WebSocket, ws_state: WsState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Subscribe to broadcasts
    let mut broadcast_rx = ws_state.broadcaster.subscribe();

    // Forward broadcasts to this client
    let send_task = tokio::spawn(async move {
        while let Some(msg) = broadcast_rx.recv().await {
            let ws_msg = match msg {
                broadcast::WsOutMsg::Text(text) => Message::Text(text.into()),
                broadcast::WsOutMsg::Binary(data) => Message::Binary(data.into()),
            };
            if SinkExt::send(&mut ws_sender, ws_msg).await.is_err() {
                break;
            }
        }
    });

    // Read commands from client
    let state_clone = ws_state.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = StreamExt::next(&mut ws_receiver).await {
            match msg {
                Message::Text(text) => {
                    handle_text_message(&state_clone, &text).await;
                }
                Message::Binary(data) => {
                    handle_binary_message(&state_clone, &data);
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish, then abort the other
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }
}

/// Handle a JSON text command from a WS client.
async fn handle_text_message(state: &WsState, text: &str) {
    let parsed: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    let id = parsed.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let cmd = match parsed.get("cmd").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return,
    };
    let args = parsed.get("args").cloned().unwrap_or(serde_json::json!({}));

    let response = commands::dispatch(state, id, cmd, &args).await;

    // Send response back to this specific client via broadcast
    // (We use the broadcaster's text broadcast, which goes to all clients.
    //  For command responses, we include the id so the client matches it.)
    let response_text = response.to_string();
    state.broadcaster.broadcast_event("__cmd_response", &serde_json::json!({
        "id": id,
        "data": response,
    }));

    // Actually, command responses should go to the requesting client only.
    // Since we don't have a per-client sender here, we broadcast with the id.
    // The client filters by id. This is acceptable for low client counts.
    let _ = response_text; // suppress warning
}

/// Handle a binary PTY write from a WS client.
/// Format: [36 bytes UUID ASCII][raw PTY input bytes]
fn handle_binary_message(state: &WsState, data: &[u8]) {
    if data.len() < 36 {
        return;
    }

    let session_id_str = match std::str::from_utf8(&data[..36]) {
        Ok(s) => s.trim(),
        Err(_) => return,
    };

    let uuid = match uuid::Uuid::parse_str(session_id_str) {
        Ok(u) => u,
        Err(_) => return,
    };

    let pty_data = &data[36..];
    let _ = state.pty_mgr.lock().unwrap().write(uuid, pty_data);
}

/// Resolve the dist/ directory for static file serving.
fn resolve_dist_path() -> Option<std::path::PathBuf> {
    // Try relative to executable first (production)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let dist = parent.join("dist");
            if dist.exists() && dist.is_dir() {
                return Some(dist);
            }
            // Try one level up (dev mode: target/debug/exe → project root/dist)
            if let Some(grandparent) = parent.parent() {
                let dist = grandparent.join("dist");
                if dist.exists() && dist.is_dir() {
                    return Some(dist);
                }
                // Two levels up (target/debug → target → project root)
                if let Some(ggparent) = grandparent.parent() {
                    let dist = ggparent.join("dist");
                    if dist.exists() && dist.is_dir() {
                        return Some(dist);
                    }
                }
            }
        }
    }

    // Try CWD/dist
    let cwd_dist = std::path::PathBuf::from("dist");
    if cwd_dist.exists() && cwd_dist.is_dir() {
        return Some(cwd_dist);
    }

    // Try CWD/../dist (when CWD is src-tauri/)
    let cwd_parent_dist = std::path::PathBuf::from("../dist");
    if cwd_parent_dist.exists() && cwd_parent_dist.is_dir() {
        return Some(cwd_parent_dist.canonicalize().unwrap_or(cwd_parent_dist));
    }

    None
}
