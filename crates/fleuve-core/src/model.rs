//! Types partagés entre le moteur, l'interface et l'extension.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// État d'un téléchargement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// En file d'attente, démarrera dès qu'une place se libère.
    Queued,
    /// Programmé pour une heure précise.
    Scheduled,
    Downloading,
    /// Flux HLS : assemblage des morceaux en un seul fichier.
    Assembling,
    Paused,
    Completed,
    Failed,
}

impl Status {
    pub fn is_running(self) -> bool {
        matches!(self, Status::Downloading | Status::Assembling)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Fichier classique (HTTP/HTTPS), découpé en segments.
    File,
    /// Liste de lecture HLS (.m3u8) : morceaux téléchargés puis concaténés.
    Hls,
    /// Page vidéo d'un site (YouTube…) : extraction confiée à yt-dlp.
    Site,
}

/// Une plage d'octets `[start, end)` du fichier. `pos` est la prochaine
/// position à écrire : la partie `[start, pos)` est déjà reçue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
    /// Une connexion travaille actuellement sur ce segment.
    #[serde(default)]
    pub active: bool,
}

impl Segment {
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
    pub fn received(&self) -> u64 {
        self.pos.saturating_sub(self.start)
    }
}

/// En-têtes transmis par le navigateur pour reproduire la requête d'origine.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequestHeaders {
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// Ce qui est sauvegardé sur disque pour chaque téléchargement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub final_url: Option<String>,
    pub kind: Kind,
    pub file_name: String,
    /// Le nom est définitif (choisi par l'utilisateur ou fixé au premier démarrage).
    #[serde(default)]
    pub name_fixed: bool,
    pub dest_dir: PathBuf,
    pub category: String,
    pub status: Status,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub downloaded: u64,
    #[serde(default)]
    pub resumable: bool,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub hls_total_parts: u32,
    #[serde(default)]
    pub hls_done: Vec<u32>,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub headers: RequestHeaders,
    pub created_at: i64,
    #[serde(default)]
    pub scheduled_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Hauteur d'image maximale pour une vidéo (720 = 720p) ; `None` = la meilleure.
    #[serde(default)]
    pub max_height: Option<u32>,
    /// Vidéo en attente du choix de qualité (réglage « Demander à chaque fois »).
    #[serde(default)]
    pub quality_pending: bool,
}

impl DownloadRecord {
    /// Octets reçus, calculés à partir des segments quand il y en a.
    pub fn received(&self) -> u64 {
        if self.kind == Kind::File && self.resumable && !self.segments.is_empty() {
            self.segments.iter().map(Segment::received).sum()
        } else {
            self.downloaded
        }
    }
}

/// Vue envoyée à l'interface (plus légère que l'enregistrement complet).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadView {
    pub id: String,
    pub url: String,
    pub kind: Kind,
    pub file_name: String,
    pub dest_dir: PathBuf,
    pub category: String,
    pub status: Status,
    pub total: Option<u64>,
    pub received: u64,
    pub resumable: bool,
    /// Progression entre 0 et 1, ou `None` si la taille est inconnue.
    pub progress: Option<f64>,
    /// Octets par seconde.
    pub speed: f64,
    pub eta_secs: Option<u64>,
    pub connections: u32,
    pub segments: Vec<Segment>,
    pub hls_total_parts: u32,
    pub hls_done_parts: u32,
    pub created_at: i64,
    pub scheduled_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub max_height: Option<u32>,
    pub quality_pending: bool,
}

/// Une qualité proposée pour une vidéo.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QualityOption {
    /// Hauteur d'image (1080 pour 1080p).
    pub height: u32,
    /// Taille estimée en octets, quand le site la donne.
    pub size: Option<u64>,
}

/// Lit un choix de qualité : « best » (ou vide) = la meilleure, « 720 » = 720p au plus.
pub fn parse_quality(q: &str) -> Option<u32> {
    q.trim().trim_end_matches('p').parse().ok().filter(|h| *h > 0)
}

fn default_video_quality() -> String {
    "best".into()
}

/// Demande d'ajout (depuis l'interface ou l'extension).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub url: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub dest_dir: Option<PathBuf>,
    #[serde(default)]
    pub headers: RequestHeaders,
    /// Horodatage Unix (secondes) de démarrage programmé.
    #[serde(default)]
    pub scheduled_at: Option<i64>,
    #[serde(default)]
    pub start_paused: bool,
    /// Forcer le type ; sinon détecté à partir de l'URL.
    #[serde(default)]
    pub kind: Option<Kind>,
    /// Qualité vidéo : « best » ou une hauteur (« 720 »). Absent : selon les réglages.
    #[serde(default)]
    pub quality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: PathBuf,
    /// Connexions simultanées par fichier (1 à 32).
    pub connections: u32,
    /// Téléchargements simultanés.
    pub max_concurrent: u32,
    /// Limite de débit totale en Ko/s ; 0 = illimité.
    pub speed_limit_kbps: u64,
    /// Ranger dans des sous-dossiers par catégorie (Vidéos, Musique…).
    pub sort_by_category: bool,
    /// Jeton partagé avec l'extension navigateur.
    pub extension_token: String,
    pub server_port: u16,
    /// Qualité des vidéos : « ask » (demander), « best », ou une hauteur maximale (« 1080 »).
    #[serde(default = "default_video_quality")]
    pub video_quality: String,
}

impl Settings {
    pub fn with_dir(download_dir: PathBuf) -> Self {
        Settings {
            download_dir,
            connections: 8,
            max_concurrent: 3,
            speed_limit_kbps: 0,
            sort_by_category: true,
            extension_token: uuid::Uuid::new_v4().simple().to_string(),
            server_port: 47821,
            video_quality: default_video_quality(),
        }
    }
}
