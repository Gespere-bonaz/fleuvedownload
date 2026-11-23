//! Petit serveur HTTP local (127.0.0.1 uniquement) qui reçoit les
//! téléchargements envoyés par l'extension du navigateur.
//!
//! Sécurité : écoute seulement en local, exige le jeton secret partagé
//! (`X-Fleuve-Token`) et refuse toute requête venant d'une page web
//! (en-tête `Origin` autre qu'une extension).

use crate::model::{AddRequest, Kind, RequestHeaders};
use crate::pairing::PairPoll;
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
    /// Qualité voulue pour une vidéo : « best », « ask » ou une hauteur (« 720 »).
    /// Absent : réglage de Fleuve.
    #[serde(default)]
    quality: Option<String>,
    /// Titre de la vidéo (titre de la page), utilisé comme nom de fichier.
    #[serde(default)]
    title: Option<String>,
}

/// Demande des qualités disponibles pour un lien vidéo.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QualitiesQuery {
    url: String,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    cookie: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
}

fn yes() -> bool {
    true
}

const EXTENSION_ORIGINS: [&str; 3] = ["chrome-extension://", "moz-extension://", "safari-web-extension://"];

fn check(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    if let Some(origin) = headers.get("origin").and_then(|o| o.to_str().ok()) {
        let ok = EXTENSION_ORIGINS.iter().any(|p| origin.starts_with(p));
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
        quality: body.quality.filter(|q| !q.trim().is_empty()),
        title: body.title,
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

/// Qualités proposées par un lien vidéo (flux HLS ou site vidéo), de la
/// meilleure à la plus basse. Liste vide : une seule qualité, ou pas une vidéo.
async fn qualities(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<QualitiesQuery>,
) -> (StatusCode, Json<Value>) {
    if let Err(e) = check(&state, &headers) {
        return e;
    }
    if !Manager::is_video_url(&body.url) {
        return (StatusCode::OK, Json(json!({"ok": true, "qualities": []})));
    }
    let h = RequestHeaders { referer: body.referer, cookie: body.cookie, user_agent: body.user_agent };
    match state.manager.probe_qualities(&body.url, &h).await {
        Ok(list) => (StatusCode::OK, Json(json!({"ok": true, "qualities": list}))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": e.to_string()}))),
    }
}

/// L'origine d'une extension de navigateur. Contrairement aux autres routes,
/// l'appairage exige cet en-tête : un programme local qui n'en envoie pas
/// (curl, script) est refusé d'emblée.
fn extension_origin(headers: &HeaderMap) -> Result<String, (StatusCode, Json<Value>)> {
    let refused = || (StatusCode::FORBIDDEN, Json(json!({"ok": false, "error": "origine refusée"})));
    let origin = headers.get("origin").and_then(|o| o.to_str().ok()).ok_or_else(refused)?;
    if !EXTENSION_ORIGINS.iter().any(|p| origin.starts_with(p)) {
        return Err(refused());
    }
    Ok(origin.to_string())
}

/// Début de l'appairage : crée une demande, que l'utilisateur doit accepter
/// dans Fleuve après avoir comparé le code. Pas de jeton ici (c'est le but).
async fn pair_start(State(state): State<AppState>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    let origin = match extension_origin(&headers) {
        Ok(o) => o,
        Err(e) => return e,
    };
    match state.manager.pair_start(&origin) {
        Ok((id, code)) => (
            StatusCode::OK,
            Json(json!({"ok": true, "id": id, "code": code, "expiresIn": crate::pairing::TTL.as_secs()})),
        ),
        Err(e) => (StatusCode::TOO_MANY_REQUESTS, Json(json!({"ok": false, "error": e.to_string()}))),
    }
}

#[derive(Debug, Deserialize)]
struct PairStatusQuery {
    id: String,
}

/// L'extension demande où en est sa demande. Le jeton n'est renvoyé qu'après
/// l'accord de l'utilisateur, une seule fois, à l'extension qui a fait la demande.
async fn pair_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PairStatusQuery>,
) -> (StatusCode, Json<Value>) {
    let origin = match extension_origin(&headers) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let reply = match state.manager.pair_poll(&body.id, &origin) {
        PairPoll::Pending => json!({"ok": true, "state": "pending"}),
        PairPoll::Approved(token) => json!({"ok": true, "state": "approved", "token": token}),
        PairPoll::Denied => json!({"ok": true, "state": "denied"}),
        PairPoll::Expired => json!({"ok": true, "state": "expired"}),
    };
    (StatusCode::OK, Json(reply))
}

pub fn router(manager: Manager, on_added: OnAdded) -> Router {
    Router::new()
        .route("/ping", get(ping))
        .route("/add", post(add))
        .route("/qualities", post(qualities))
        .route("/pair/start", post(pair_start))
        .route("/pair/status", post(pair_status))
        .with_state(AppState { manager, on_added })
}

/// Démarre le serveur sur 127.0.0.1:`port`. Ne rend la main qu'en cas d'erreur.
pub async fn serve(manager: Manager, port: u16, on_added: OnAdded) -> anyhow::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(manager, on_added)).await?;
    Ok(())
}
