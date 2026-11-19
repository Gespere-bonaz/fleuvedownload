//! Gestionnaire : liste des téléchargements, file d'attente, planificateur,
//! sauvegarde sur disque et calcul des vitesses.

use crate::limiter::RateLimiter;
use crate::model::*;
use crate::util;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

const STATE_FILE: &str = "downloads.json";
const SETTINGS_FILE: &str = "settings.json";

/// Un téléchargement en mémoire.
pub(crate) struct Item {
    pub id: String,
    pub rec: Mutex<DownloadRecord>,
    pub cancel: Mutex<Option<CancellationToken>>,
    pub active_conns: AtomicU32,
    /// Une tâche moteur tourne encore pour cet élément (même en cours d'arrêt).
    pub alive: AtomicBool,
    speed: Mutex<SpeedMeter>,
}

#[derive(Default)]
struct SpeedMeter {
    last_bytes: u64,
    last_at: Option<Instant>,
    speed: f64,
}

impl Item {
    fn new(rec: DownloadRecord) -> Arc<Self> {
        Arc::new(Item {
            id: rec.id.clone(),
            rec: Mutex::new(rec),
            cancel: Mutex::new(None),
            active_conns: AtomicU32::new(0),
            alive: AtomicBool::new(false),
            speed: Mutex::new(SpeedMeter::default()),
        })
    }

    pub fn status(&self) -> Status {
        self.rec.lock().unwrap().status
    }

    fn view(&self) -> DownloadView {
        let r = self.rec.lock().unwrap();
        let speed = if r.status.is_running() {
            self.speed.lock().unwrap().speed
        } else {
            0.0
        };
        let received = r.received();
        let progress = match (r.kind, r.total) {
            (Kind::Hls, _) if r.hls_total_parts > 0 => {
                Some(r.hls_done.len() as f64 / r.hls_total_parts as f64)
            }
            (_, Some(t)) if t > 0 => Some((received as f64 / t as f64).min(1.0)),
            (_, Some(0)) if r.status == Status::Completed => Some(1.0),
            _ if r.status == Status::Completed => Some(1.0),
            _ => None,
        };
        let eta_secs = match (r.total, r.kind) {
            (Some(t), Kind::File | Kind::Site) if speed > 1.0 && t > received => {
                Some(((t - received) as f64 / speed) as u64)
            }
            (_, Kind::Hls) if speed > 1.0 && r.hls_total_parts > 0 && !r.hls_done.is_empty() => {
                // Estimation : taille moyenne des morceaux déjà reçus.
                let avg = received as f64 / r.hls_done.len() as f64;
                let left = (r.hls_total_parts as usize - r.hls_done.len()) as f64 * avg;
                Some((left / speed) as u64)
            }
            _ => None,
        };
        let mut segments = r.segments.clone();
        segments.sort_by_key(|s| s.start);
        DownloadView {
            id: r.id.clone(),
            url: r.url.clone(),
            kind: r.kind,
            file_name: r.file_name.clone(),
            dest_dir: r.dest_dir.clone(),
            category: r.category.clone(),
            status: r.status,
            total: r.total,
            received,
            resumable: r.resumable,
            progress,
            speed,
            eta_secs,
            connections: self.active_conns.load(Ordering::Relaxed),
            segments,
            hls_total_parts: r.hls_total_parts,
            hls_done_parts: r.hls_done.len() as u32,
            max_height: r.max_height,
            quality_pending: r.quality_pending,
            created_at: r.created_at,
            scheduled_at: r.scheduled_at,
            completed_at: r.completed_at,
            error: r.error.clone(),
        }
    }
}

pub(crate) struct Inner {
    pub client: reqwest::Client,
    pub limiter: RateLimiter,
    pub settings: RwLock<Settings>,
    pub items: Mutex<Vec<Arc<Item>>>,
    data_dir: PathBuf,
    wake: Notify,
    dirty: AtomicBool,
    changes: broadcast::Sender<()>,
}

impl Inner {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        let _ = self.changes.send(());
    }

    pub fn save_state(&self) -> Result<()> {
        let records: Vec<DownloadRecord> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.rec.lock().unwrap().clone())
            .collect();
        write_json_atomic(&self.data_dir.join(STATE_FILE), &records)
    }

    fn save_settings(&self) -> Result<()> {
        let s = self.settings();
        write_json_atomic(&self.data_dir.join(SETTINGS_FILE), &s)
    }

    /// Démarre ce qui doit l'être : programmés arrivés à échéance, puis la file
    /// d'attente dans l'ordre d'ajout, sans dépasser le maximum simultané.
    fn tick(self: &Arc<Self>) {
        let now = util::now_secs();
        let max = self.settings().max_concurrent.max(1) as usize;
        let items = self.items.lock().unwrap().clone();
        let mut changed = false;
        for item in &items {
            let mut r = item.rec.lock().unwrap();
            if r.status == Status::Scheduled && r.scheduled_at.map_or(true, |t| t <= now) {
                r.status = Status::Queued;
                changed = true;
            }
        }
        let mut running = items.iter().filter(|i| i.status().is_running()).count();
        for item in &items {
            if running >= max {
                break;
            }
            // On attend la fin de l'ancienne tâche (pause puis reprise rapide).
            if item.status() == Status::Queued && !item.alive.load(Ordering::SeqCst) {
                self.start(item.clone());
                running += 1;
                changed = true;
            }
        }
        if changed {
            self.mark_dirty();
        }
    }

    fn start(self: &Arc<Self>, item: Arc<Item>) {
        let token = CancellationToken::new();
        {
            let mut r = item.rec.lock().unwrap();
            r.status = Status::Downloading;
            r.error = None;
        }
        *item.cancel.lock().unwrap() = Some(token.clone());
        *item.speed.lock().unwrap() = SpeedMeter::default();
        item.alive.store(true, Ordering::SeqCst);
        let inner = self.clone();
        tokio::spawn(async move {
            crate::engine::run(inner.clone(), item.clone(), token).await;
            item.alive.store(false, Ordering::SeqCst);
            let _ = inner.save_state();
            inner.mark_dirty();
            inner.wake.notify_one();
        });
    }

    fn find(&self, id: &str) -> Option<Arc<Item>> {
        self.items.lock().unwrap().iter().find(|i| i.id == id).cloned()
    }

    /// Mesure les vitesses (moyenne glissante sur ~2 s).
    fn measure_speeds(&self) -> bool {
        let items = self.items.lock().unwrap().clone();
        let mut any_running = false;
        for item in items {
            let (running, received) = {
                let r = item.rec.lock().unwrap();
                (r.status.is_running(), r.received())
            };
            if !running {
                continue;
            }
            any_running = true;
            let mut m = item.speed.lock().unwrap();
            let now = Instant::now();
            if let Some(last) = m.last_at {
                let dt = now.duration_since(last).as_secs_f64();
                if dt > 0.0 {
                    let inst = received.saturating_sub(m.last_bytes) as f64 / dt;
                    m.speed = if m.speed == 0.0 { inst } else { m.speed * 0.75 + inst * 0.25 };
                }
            }
            m.last_at = Some(now);
            m.last_bytes = received;
        }
        any_running
    }
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, data).with_context(|| format!("écriture de {}", tmp.display()))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Point d'entrée public du moteur. Clonable à volonté (compteur de références).
#[derive(Clone)]
pub struct Manager {
    pub(crate) inner: Arc<Inner>,
}

impl Manager {
    /// Charge l'état depuis `data_dir` et lance le planificateur.
    /// Doit être appelé dans un runtime Tokio.
    pub async fn new(data_dir: PathBuf, default_download_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let settings: Settings = std::fs::read(data_dir.join(SETTINGS_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_else(|| Settings::with_dir(default_download_dir));
        let mut records: Vec<DownloadRecord> = std::fs::read(data_dir.join(STATE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Après un redémarrage, ce qui tournait repart automatiquement.
        for r in &mut records {
            if r.status.is_running() {
                r.status = Status::Queued;
            }
            for s in &mut r.segments {
                s.active = false;
            }
        }

        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) Fleuve/",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(32)
            .build()?;

        let (changes, _) = broadcast::channel(64);
        let inner = Arc::new(Inner {
            client,
            limiter: RateLimiter::new(settings.speed_limit_kbps * 1024),
            settings: RwLock::new(settings),
            items: Mutex::new(records.into_iter().map(Item::new).collect()),
            data_dir,
            wake: Notify::new(),
            dirty: AtomicBool::new(false),
            changes,
        });
        inner.save_settings()?;

        // Planificateur : réveillé à chaque changement, sinon toutes les secondes.
        let sched = inner.clone();
        tokio::spawn(async move {
            loop {
                sched.tick();
                tokio::select! {
                    _ = sched.wake.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        });

        // Mesure des vitesses et notification de l'interface toutes les 500 ms.
        let ticker = inner.clone();
        tokio::spawn(async move {
            let mut n: u32 = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                n = n.wrapping_add(1);
                let running = ticker.measure_speeds();
                if running {
                    let _ = ticker.changes.send(());
                }
                // Sauvegarde toutes les 2 s pendant un téléchargement, ou si modifié.
                if n % 4 == 0 && (running || ticker.dirty.swap(false, Ordering::Relaxed)) {
                    let _ = ticker.save_state();
                }
            }
        });

        Ok(Manager { inner })
    }

    /// Reçoit un signal à chaque changement visible (pour rafraîchir l'interface).
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.inner.changes.subscribe()
    }

    pub fn list(&self) -> Vec<DownloadView> {
        self.inner
            .items
            .lock()
            .unwrap()
            .iter()
            .map(|i| i.view())
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<DownloadView> {
        self.inner.find(id).map(|i| i.view())
    }

    pub fn add(&self, req: AddRequest) -> Result<String> {
        let parsed = url::Url::parse(req.url.trim()).map_err(|_| anyhow!("Adresse invalide : {}", req.url))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(anyhow!("Seuls les liens http et https sont pris en charge"));
        }
        let url = parsed.to_string();
        let settings = self.inner.settings();
        let kind = req
            .kind
            .unwrap_or(if util::looks_like_hls(&url) {
                Kind::Hls
            } else if crate::site::is_video_site(&url) {
                Kind::Site
            } else {
                Kind::File
            });
        let name_fixed = req.file_name.as_deref().map_or(false, |n| !n.trim().is_empty());
        let mut file_name = match &req.file_name {
            Some(n) if !n.trim().is_empty() => util::sanitize_file_name(n),
            _ => util::file_name_from_url(&url),
        };
        if kind == Kind::Hls && !name_fixed {
            file_name = util::with_extension(&file_name, "ts");
        }
        if kind == Kind::Site && !name_fixed {
            file_name = "Vidéo (recherche du titre…)".into();
        }
        let category = if kind == Kind::Site {
            "Vidéos".to_string()
        } else {
            util::category_for(&file_name).to_string()
        };
        let dest_dir = req.dest_dir.clone().unwrap_or_else(|| {
            if settings.sort_by_category {
                settings.download_dir.join(&category)
            } else {
                settings.download_dir.clone()
            }
        });
        // Qualité : choix explicite, sinon réglage ; « ask » met la vidéo en attente.
        let is_video = matches!(kind, Kind::Hls | Kind::Site);
        let wanted = req.quality.clone().unwrap_or_else(|| settings.video_quality.clone());
        let quality_pending = is_video && wanted == "ask";
        let max_height = if is_video { crate::model::parse_quality(&wanted) } else { None };
        let status = if req.start_paused || quality_pending {
            Status::Paused
        } else if req.scheduled_at.map_or(false, |t| t > util::now_secs()) {
            Status::Scheduled
        } else {
            Status::Queued
        };
        let rec = DownloadRecord {
            id: uuid::Uuid::new_v4().to_string(),
            url,
            final_url: None,
            kind,
            file_name,
            name_fixed,
            dest_dir,
            category,
            status,
            total: None,
            downloaded: 0,
            resumable: false,
            segments: vec![],
            hls_total_parts: 0,
            hls_done: vec![],
            etag: None,
            last_modified: None,
            headers: req.headers,
            created_at: util::now_secs(),
            scheduled_at: req.scheduled_at,
            completed_at: None,
            error: None,
            max_height,
            quality_pending,
        };
        let id = rec.id.clone();
        self.inner.items.lock().unwrap().push(Item::new(rec));
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(id)
    }

    pub fn pause(&self, id: &str) -> Result<()> {
        let item = self.inner.find(id).ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
        {
            let mut r = item.rec.lock().unwrap();
            if r.status == Status::Completed {
                return Ok(());
            }
            r.status = Status::Paused;
        }
        if let Some(t) = item.cancel.lock().unwrap().take() {
            t.cancel();
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Arrête tout : téléchargements en cours, en file d'attente et programmés.
    /// Rien ne redémarre seul ensuite ; « Reprendre » relance chacun.
    pub fn pause_all(&self) -> Result<usize> {
        let items = self.inner.items.lock().unwrap().clone();
        let mut n = 0;
        for item in items {
            {
                let mut r = item.rec.lock().unwrap();
                if !(r.status.is_running() || matches!(r.status, Status::Queued | Status::Scheduled)) {
                    continue;
                }
                r.status = Status::Paused;
            }
            if let Some(t) = item.cancel.lock().unwrap().take() {
                t.cancel();
            }
            n += 1;
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        Ok(n)
    }

    /// À la fermeture de l'app : coupe les connexions et les programmes
    /// externes (yt-dlp) pour qu'aucun téléchargement ne continue en cachette.
    /// Ce qui tournait reprendra au prochain lancement.
    pub async fn shutdown(&self) {
        let items = self.inner.items.lock().unwrap().clone();
        for item in &items {
            if let Some(t) = item.cancel.lock().unwrap().take() {
                t.cancel();
            }
        }
        for _ in 0..30 {
            if !items.iter().any(|i| i.alive.load(Ordering::SeqCst)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = self.inner.save_state();
    }

    /// Remet en file un téléchargement en pause, en échec ou programmé.
    pub fn resume(&self, id: &str) -> Result<()> {
        let item = self.inner.find(id).ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
        {
            let mut r = item.rec.lock().unwrap();
            match r.status {
                Status::Paused | Status::Failed | Status::Scheduled => {
                    r.status = Status::Queued;
                    r.quality_pending = false;
                    r.scheduled_at = None;
                    r.error = None;
                }
                _ => return Ok(()),
            }
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Fixe la qualité d'une vidéo (« best » ou « 720 ») et la démarre si elle attendait ce choix.
    pub fn set_quality(&self, id: &str, quality: &str) -> Result<()> {
        let item = self.inner.find(id).ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
        {
            let mut r = item.rec.lock().unwrap();
            r.max_height = crate::model::parse_quality(quality);
            if r.quality_pending {
                r.quality_pending = false;
                if r.status == Status::Paused {
                    r.status = Status::Queued;
                }
            }
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Qualités disponibles pour une vidéo, de la meilleure à la plus basse.
    /// Liste vide : une seule qualité, ou ce n'est pas une vidéo.
    pub async fn probe_qualities(&self, url: &str, headers: &RequestHeaders) -> Result<Vec<crate::model::QualityOption>> {
        if util::looks_like_hls(url) {
            crate::hls::probe_qualities(&self.inner, url, headers).await
        } else if crate::site::is_video_site(url) {
            crate::site::probe_qualities(url, headers).await
        } else {
            Ok(vec![])
        }
    }

    /// Qualités disponibles pour un téléchargement déjà ajouté.
    pub async fn probe_qualities_for(&self, id: &str) -> Result<Vec<crate::model::QualityOption>> {
        let (url, headers) = {
            let item = self.inner.find(id).ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
            let r = item.rec.lock().unwrap();
            (r.url.clone(), r.headers.clone())
        };
        self.probe_qualities(&url, &headers).await
    }

    /// Le lien mène-t-il à une vidéo (flux HLS ou site vidéo) ?
    pub fn is_video_url(url: &str) -> bool {
        util::looks_like_hls(url) || crate::site::is_video_site(url)
    }

    /// Programme (ou reprogramme) un téléchargement non terminé.
    pub fn schedule(&self, id: &str, at: i64) -> Result<()> {
        let item = self.inner.find(id).ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
        if item.status().is_running() {
            self.pause(id)?;
        }
        {
            let mut r = item.rec.lock().unwrap();
            if r.status == Status::Completed {
                return Err(anyhow!("Ce téléchargement est déjà terminé"));
            }
            r.scheduled_at = Some(at);
            r.status = Status::Scheduled;
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Supprime de la liste ; `delete_file` efface aussi le fichier (ou le partiel).
    pub fn remove(&self, id: &str, delete_file: bool) -> Result<()> {
        let item = {
            let mut items = self.inner.items.lock().unwrap();
            let pos = items
                .iter()
                .position(|i| i.id == id)
                .ok_or_else(|| anyhow!("Téléchargement introuvable"))?;
            items.remove(pos)
        };
        if let Some(t) = item.cancel.lock().unwrap().take() {
            t.cancel();
        }
        let r = item.rec.lock().unwrap().clone();
        // Le fichier temporaire n'est jamais utile une fois retiré de la liste.
        let _ = std::fs::remove_file(util::part_path(&r.dest_dir, &r.file_name));
        let _ = std::fs::remove_dir_all(crate::hls::temp_dir(&r.dest_dir, &r.file_name));
        if r.kind == Kind::Site && r.status != Status::Completed {
            crate::site::remove_partials(&r.dest_dir, &r.file_name);
        }
        if delete_file && r.status == Status::Completed {
            let _ = std::fs::remove_file(r.dest_dir.join(&r.file_name));
        }
        self.inner.save_state()?;
        self.inner.mark_dirty();
        self.inner.wake.notify_one();
        Ok(())
    }

    /// Vide la liste. `all = false` : seulement les téléchargements terminés
    /// ou en échec ; `all = true` : tout, en arrêtant ceux en cours. Les
    /// fichiers terminés restent sur le disque ; les fichiers partiels sont
    /// supprimés comme pour un retrait.
    pub fn clear(&self, all: bool) -> Result<usize> {
        let ids: Vec<String> = self
            .inner
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|i| all || matches!(i.rec.lock().unwrap().status, Status::Completed | Status::Failed))
            .map(|i| i.id.clone())
            .collect();
        for id in &ids {
            self.remove(id, false)?;
        }
        Ok(ids.len())
    }

    /// Chemin du fichier final (ou prévu).
    pub fn file_path(&self, id: &str) -> Option<PathBuf> {
        self.inner.find(id).map(|i| {
            let r = i.rec.lock().unwrap();
            r.dest_dir.join(&r.file_name)
        })
    }

    pub fn settings(&self) -> Settings {
        self.inner.settings()
    }

    pub fn set_settings(&self, mut s: Settings) -> Result<Settings> {
        s.connections = s.connections.clamp(1, 32);
        s.max_concurrent = s.max_concurrent.clamp(1, 10);
        if s.extension_token.trim().len() < 16 {
            s.extension_token = uuid::Uuid::new_v4().simple().to_string();
        }
        self.inner.limiter.set_rate(s.speed_limit_kbps * 1024);
        *self.inner.settings.write().unwrap() = s.clone();
        self.inner.save_settings()?;
        self.inner.wake.notify_one();
        Ok(s)
    }
}

#[cfg(test)]
pub(crate) fn test_item(segments: Vec<Segment>) -> Arc<Item> {
    Item::new(DownloadRecord {
        id: "t".into(),
        url: "http://localhost/f".into(),
        final_url: None,
        kind: Kind::File,
        file_name: "f".into(),
        name_fixed: true,
        dest_dir: PathBuf::from("/tmp"),
        category: "Autres".into(),
        status: Status::Downloading,
        total: segments.iter().map(|s| s.end).max(),
        downloaded: 0,
        resumable: true,
        segments,
        hls_total_parts: 0,
        hls_done: vec![],
        etag: None,
        last_modified: None,
        headers: RequestHeaders::default(),
        created_at: 0,
        scheduled_at: None,
        completed_at: None,
        error: None,
        max_height: None,
        quality_pending: false,
    })
}
