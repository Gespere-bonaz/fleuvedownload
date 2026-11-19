//! Petit serveur HTTP local (127.0.0.1 uniquement) qui reçoit les
//! téléchargements envoyés par l'extension du navigateur.
//!
//! Sécurité : écoute seulement en local, exige le jeton secret partagé
//! (`X-Fleuve-Token`) et refuse toute requête venant d'une page web
//! (en-tête `Origin` autre qu'une extension).

use crate::model::{AddRequest, Kind, RequestHeaders};
use crate::Manager;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// Appelé après chaque ajout : (identifiant, faut-il afficher la fenêtre de Fleuve).
pub type OnAdded = Arc<dyn Fn(String, bool) + Send + Sync>;

#[derive(Clone)]
struct AppState {
    manager: Manager,
    on_added: OnAdded,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionAdd {
    url: String,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    cookie: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
    /// "hls" pour forcer le moteur vidéo.
    #[serde(default)]
    kind: Option<String>,
    /// Mettre Fleuve au premier plan (vrai par défaut). L'extension envoie
    /// `false` quand l'utilisateur a validé dans sa propre fenêtre.
    #[serde(default = "yes")]
    focus: bool,
    /// Ajouter sans démarrer (« Télécharger plus tard »).
    #[serde(default)]
    start_paused: bool,
}

fn yes() -> bool {
    true
}

fn check(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    if let Some(origin) = headers.get("origin").and_then(|o| o.to_str().ok()) {
        let ok = ["chrome-extension://", "moz-extension://", "safari-web-extension://"]
            .iter()
            .any(|p| origin.starts_with(p));
        if !ok {
            return Err((StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "origine refusée"}))));
        }
    }
    let expected = state.manager.settings().extension_token;
    let given = headers.get("x-fleuve-token").and_then(|t| t.to_str().ok()).unwrap_or("");
    if given.is_empty() || !constant_time_eq(given.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::UNAUTHORIZED, Json(json!({"ok": false, "error": "jeton invalide"}))));
    }
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn ping(State(state): State<AppState>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    match check(&state, &headers) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "app": "fleuve",
                "version": env!("CARGO_PKG_VERSION"),
                "downloadDir": state.manager.settings().download_dir,
            })),
        ),
        Err(e) => e,
    }
}

async fn add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ExtensionAdd>,
) -> (StatusCode, Json<Value>) {
    if let Err(e) = check(&state, &headers) {
        return e;
    }
    let req = AddRequest {
        url: body.url,
        file_name: body.file_name,
        headers: RequestHeaders {
            referer: body.referer,
            cookie: body.cookie,
            user_agent: body.user_agent,
        },
        kind: match body.kind.as_deref() {
            Some("hls") => Some(Kind::Hls),
            Some("file") => Some(Kind::File),
            Some("site") => Some(Kind::Site),
            _ => None,
        },
        start_paused: body.start_paused,
        ..Default::default()
    };
    match state.manager.add(req) {
        Ok(id) => {
            // Une vidéo qui attend le choix de qualité affiche toujours Fleuve.
            let pending = state.manager.list().iter().any(|d| d.id == id && d.quality_pending);
            (state.on_added)(id.clone(), body.focus || pending);
            (StatusCode::OK, Json(json!({"ok": true, "id": id})))
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e.to_string()}))),
    }
}

pub fn router(manager: Manager, on_added: OnAdded) -> Router {
    Router::new()
        .route("/ping", get(ping))
        .route("/add", post(add))
        .with_state(AppState { manager, on_added })
}

/// Démarre le serveur sur 127.0.0.1:`port`. Ne rend la main qu'en cas d'erreur.
pub async fn serve(manager: Manager, port: u16, on_added: OnAdded) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(manager, on_added)).await?;
    Ok(())
}
