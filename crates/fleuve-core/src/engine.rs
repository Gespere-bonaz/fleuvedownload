//! Moteur de téléchargement de fichiers : sonde, segmentation dynamique
//! (règle de division en deux d'IDM), reprise et flux unique de secours.

use crate::manager::{Inner, Item};
use crate::model::*;
use crate::util;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::StatusCode;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// On ne coupe pas un segment si chaque moitié ferait moins de 1 Mo.
pub const MIN_SPLIT: u64 = 1024 * 1024;
/// Délai maximal sans recevoir d'octets avant de relancer la connexion.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 5;

/// Erreur définitive : inutile de réessayer.
#[derive(Debug)]
pub struct Fatal(pub String);
impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Fatal {}

#[derive(Debug)]
struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("annulé")
    }
}
impl std::error::Error for Cancelled {}

/// Lance le téléchargement et met à jour le statut final.
pub(crate) async fn run(inner: Arc<Inner>, item: Arc<Item>, token: CancellationToken) {
    let kind = item.rec.lock().unwrap().kind;
    let res = match kind {
        Kind::File => run_file(&inner, &item, &token).await,
        Kind::Hls => crate::hls::run_hls(&inner, &item, &token).await,
        Kind::Site => crate::site::run_site(&inner, &item, &token).await,
    };
    item.active_conns.store(0, Ordering::Relaxed);
    let mut r = item.rec.lock().unwrap();
    for s in &mut r.segments {
        s.active = false;
    }
    match res {
        Ok(()) => {
            r.status = Status::Completed;
            r.completed_at = Some(util::now_secs());
            r.error = None;
        }
        Err(_) if token.is_cancelled() => {
            // Pause, programmation, suppression ou fermeture de l'app : le statut
            // a déjà été fixé (à la fermeture il reste « en cours » pour reprendre
            // au prochain lancement).
        }
        Err(e) => {
            r.status = Status::Failed;
            r.error = Some(format!("{e:#}"));
        }
    }
    drop(r);
    item.cancel.lock().unwrap().take();
}

/// En-têtes de la requête d'origine (référent, cookies, agent).
pub(crate) fn request_headers(h: &RequestHeaders) -> HeaderMap {
    let mut map = HeaderMap::new();
    let mut put = |name: header::HeaderName, v: &Option<String>| {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            if let Ok(v) = HeaderValue::from_str(v) {
                map.insert(name, v);
            }
        }
    };
    put(header::REFERER, &h.referer);
    put(header::COOKIE, &h.cookie);
    put(header::USER_AGENT, &h.user_agent);
    map.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    map
}

pub(crate) async fn cancellable<T>(
    token: &CancellationToken,
    fut: impl std::future::Future<Output = T>,
) -> Result<T> {
    tokio::select! {
        _ = token.cancelled() => Err(anyhow!(Cancelled)),
        v = fut => Ok(v),
    }
}

#[derive(Debug, Default)]
pub(crate) struct Probe {
    pub total: Option<u64>,
    pub resumable: bool,
    pub final_url: String,
    pub file_name: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

/// La première demande est réessayée : un serveur lent ou une coupure
/// passagère ne doit pas faire échouer le téléchargement d'emblée.
async fn probe_retry(inner: &Inner, url: &str, headers: &HeaderMap, token: &CancellationToken) -> Result<Probe> {
    const TRIES: u32 = 4;
    let mut n = 0;
    loop {
        match cancellable(token, probe(inner, url, headers)).await? {
            Ok(p) => return Ok(p),
            Err(e) if e.downcast_ref::<Fatal>().is_some() => return Err(e),
            Err(e) => {
                n += 1;
                if n >= TRIES {
                    let timeout = e
                        .chain()
                        .any(|c| c.downcast_ref::<reqwest::Error>().is_some_and(|r| r.is_timeout() || r.is_connect()));
                    return Err(if timeout {
                        anyhow!("le serveur ne répond pas ({TRIES} essais) : réessayez plus tard ou vérifiez que le lien n'a pas expiré")
                    } else {
                        e.context(format!("échec après {TRIES} essais"))
                    });
                }
                cancellable(token, tokio::time::sleep(Duration::from_secs(2u64.pow(n)))).await?;
            }
        }
    }
}

/// Demande l'octet 0 pour connaître la taille et savoir si le serveur
/// accepte les plages (`206 Partial Content`). Plus fiable que `HEAD`,
/// que certains serveurs refusent.
pub(crate) async fn probe(inner: &Inner, url: &str, headers: &HeaderMap) -> Result<Probe> {
    let resp = inner
        .client
        .get(url)
        .headers(headers.clone())
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await
        .context("connexion au serveur impossible")?;
    let status = resp.status();
    let h = resp.headers();
    let get = |n: header::HeaderName| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut p = Probe {
        final_url: resp.url().to_string(),
        file_name: get(header::CONTENT_DISPOSITION).and_then(|v| util::file_name_from_disposition(&v)),
        etag: get(header::ETAG),
        last_modified: get(header::LAST_MODIFIED),
        content_type: get(header::CONTENT_TYPE),
        ..Default::default()
    };
    match status {
        StatusCode::PARTIAL_CONTENT => {
            // Content-Range: bytes 0-0/123456
            p.total = get(header::CONTENT_RANGE)
                .and_then(|v| v.rsplit('/').next().and_then(|t| t.trim().parse().ok()));
            p.resumable = p.total.is_some();
        }
        s if s.is_success() => {
            p.total = resp.content_length().filter(|&l| l > 0);
            p.resumable = false;
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            // Fichier vide.
            p.total = Some(0);
        }
        s => {
            return Err(anyhow!(Fatal(format!(
                "le serveur a répondu {} {}",
                s.as_u16(),
                s.canonical_reason().unwrap_or("")
            ))))
        }
    }
    Ok(p)
}

/// Fixe le nom définitif au premier démarrage (Content-Disposition, nom libre).
pub(crate) fn settle_name(item: &Item, suggested: Option<String>) -> Result<(PathBuf, String)> {
    let mut r = item.rec.lock().unwrap();
    if !r.name_fixed {
        if let Some(n) = suggested {
            r.file_name = n;
            r.category = util::category_for(&r.file_name).to_string();
        }
        std::fs::create_dir_all(&r.dest_dir)
            .with_context(|| format!("création du dossier {}", r.dest_dir.display()))?;
        r.file_name = util::unique_name(&r.dest_dir, &r.file_name);
        r.name_fixed = true;
    }
    std::fs::create_dir_all(&r.dest_dir)?;
    Ok((r.dest_dir.clone(), r.file_name.clone()))
}

async fn run_file(inner: &Arc<Inner>, item: &Arc<Item>, token: &CancellationToken) -> Result<()> {
    let (url, headers) = {
        let r = item.rec.lock().unwrap();
        (r.url.clone(), request_headers(&r.headers))
    };
    let p = probe_retry(inner, &url, &headers, token).await?;

    // Une URL qui renvoie une liste de lecture HLS bascule vers le moteur HLS.
    if p
        .content_type
        .as_deref()
        .map_or(false, |c| c.to_ascii_lowercase().contains("mpegurl"))
    {
        {
            let mut r = item.rec.lock().unwrap();
            r.kind = Kind::Hls;
            if !r.name_fixed {
                r.file_name = util::with_extension(&r.file_name, "ts");
                r.category = "Vidéos".into();
            }
        }
        return crate::hls::run_hls(inner, item, token).await;
    }

    // Une page web (ex. YouTube) n'est pas le fichier voulu : on cherche la
    // vidéo de la page avec yt-dlp plutôt que d'enregistrer le HTML.
    let is_html = p
        .content_type
        .as_deref()
        .map_or(false, |c| c.to_ascii_lowercase().starts_with("text/html"));
    let wants_html = {
        let r = item.rec.lock().unwrap();
        let n = r.file_name.to_ascii_lowercase();
        n.ends_with(".html") || n.ends_with(".htm")
    };
    if is_html && !wants_html {
        // Seulement pour une vraie page vidéo (ex. lien raccourci vers YouTube) :
        // sur une page quelconque, yt-dlp pourrait récupérer une chaîne entière.
        if !crate::site::is_video_site(&p.final_url) {
            bail!(Fatal(
                "ce lien mène à une page web, pas à un fichier. Pour une vidéo, utilisez le bouton « Télécharger cette vidéo » affiché sur la vidéo".into()
            ));
        }
        if crate::site::find_tool("yt-dlp").is_none() {
            bail!(Fatal(format!("pour les vidéos de sites, installez yt-dlp {}", crate::site::INSTALL_HINT)));
        }
        {
            let mut r = item.rec.lock().unwrap();
            r.kind = Kind::Site;
            r.name_fixed = false;
            r.category = "Vidéos".into();
        }
        return crate::site::run_site(inner, item, token).await;
    }

    let (dir, name) = settle_name(item, p.file_name.clone())?;
    let part = util::part_path(&dir, &name);
    let final_path = dir.join(&name);

    if !(p.resumable && p.total.is_some()) {
        single_stream(inner, item, token, &p, &part, &headers).await?;
    } else {
        let total = p.total.unwrap();
        // Faut-il repartir de zéro ? (taille ou version du fichier changée)
        let reset = {
            let r = item.rec.lock().unwrap();
            r.segments.is_empty()
                || !r.resumable
                || r.total != Some(total)
                || (r.etag.is_some() && p.etag.is_some() && r.etag != p.etag)
                || (r.last_modified.is_some()
                    && p.last_modified.is_some()
                    && r.last_modified != p.last_modified)
                || !part.exists()
        };
        let file = if reset {
            let f = OpenOptions::new().create(true).write(true).truncate(true).open(&part)?;
            f.set_len(total)?; // préallocation : chaque connexion écrit à sa place
            let mut r = item.rec.lock().unwrap();
            r.segments = vec![Segment { start: 0, end: total, pos: 0, active: false }];
            r.downloaded = 0;
            f
        } else {
            OpenOptions::new().write(true).open(&part)?
        };
        {
            let mut r = item.rec.lock().unwrap();
            r.total = Some(total);
            r.resumable = true;
            r.etag = p.etag.clone();
            r.last_modified = p.last_modified.clone();
            r.final_url = Some(p.final_url.clone());
        }
        if total > 0 {
            segmented(inner, item, token, Arc::new(file), &p.final_url, &headers).await?;
        }
    }

    // Terminé : le fichier temporaire prend son nom définitif.
    std::fs::rename(&part, &final_path)
        .with_context(|| format!("renommage vers {}", final_path.display()))?;
    Ok(())
}

/// Serveur sans plages : une seule connexion, reprise impossible (on recommence).
async fn single_stream(
    inner: &Arc<Inner>,
    item: &Arc<Item>,
    token: &CancellationToken,
    p: &Probe,
    part: &std::path::Path,
    headers: &HeaderMap,
) -> Result<()> {
    use std::io::Write;
    {
        let mut r = item.rec.lock().unwrap();
        r.resumable = false;
        r.total = p.total;
        r.downloaded = 0;
        r.final_url = Some(p.final_url.clone());
        r.segments = match p.total {
            Some(t) if t > 0 => vec![Segment { start: 0, end: t, pos: 0, active: true }],
            _ => vec![],
        };
    }
    let mut file = File::create(part)?;
    let resp = cancellable(token, inner.client.get(&p.final_url).headers(headers.clone()).send())
        .await?
        .context("connexion au serveur impossible")?;
    if !resp.status().is_success() {
        bail!(Fatal(format!("le serveur a répondu {}", resp.status().as_u16())));
    }
    item.active_conns.store(1, Ordering::Relaxed);
    let mut stream = resp.bytes_stream();
    loop {
        let next = cancellable(token, tokio::time::timeout(READ_TIMEOUT, stream.next())).await?;
        let chunk = match next {
            Err(_) => bail!("aucune donnée reçue depuis 30 s"),
            Ok(None) => break,
            Ok(Some(c)) => c.context("connexion interrompue")?,
        };
        inner.limiter.acquire(chunk.len() as u64).await;
        file.write_all(&chunk)?;
        let mut r = item.rec.lock().unwrap();
        r.downloaded += chunk.len() as u64;
        let d = r.downloaded;
        if let Some(s) = r.segments.first_mut() {
            s.pos = d.min(s.end);
        }
    }
    file.flush()?;
    let mut r = item.rec.lock().unwrap();
    if let Some(t) = r.total {
        if r.downloaded < t {
            bail!("fichier incomplet ({} sur {} octets)", r.downloaded, t);
        }
    } else {
        r.total = Some(r.downloaded);
    }
    Ok(())
}

struct Ctx {
    inner: Arc<Inner>,
    item: Arc<Item>,
    token: CancellationToken,
    file: Arc<File>,
    url: String,
    headers: HeaderMap,
}

/// Téléchargement multi-connexions avec segmentation dynamique.
async fn segmented(
    inner: &Arc<Inner>,
    item: &Arc<Item>,
    token: &CancellationToken,
    file: Arc<File>,
    url: &str,
    headers: &HeaderMap,
) -> Result<()> {
    // Jeton enfant : une erreur définitive arrête les connexions sans être
    // confondue avec une pause demandée par l'utilisateur.
    let work = token.child_token();
    let ctx = Arc::new(Ctx {
        inner: inner.clone(),
        item: item.clone(),
        token: work.clone(),
        file,
        url: url.to_string(),
        headers: headers.clone(),
    });
    let n = inner.settings().connections.clamp(1, 32);
    let mut set = JoinSet::new();
    for i in 0..n {
        let ctx = ctx.clone();
        // Ouverture progressive des connexions, comme IDM.
        set.spawn(async move { worker(ctx, Duration::from_millis(120 * i as u64)).await });
    }
    let mut last_err: Option<anyhow::Error> = None;
    let mut fatal: Option<anyhow::Error> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // Une erreur définitive arrête tout le monde.
                if e.downcast_ref::<Fatal>().is_some() {
                    work.cancel();
                    fatal.get_or_insert(e);
                } else {
                    last_err = Some(e);
                }
            }
            Err(e) => last_err = Some(anyhow!("tâche interrompue : {e}")),
        }
    }
    if token.is_cancelled() {
        return Err(anyhow!(Cancelled));
    }
    if let Some(e) = fatal {
        return Err(e);
    }
    let complete = item.rec.lock().unwrap().segments.iter().all(|s| s.pos >= s.end);
    if complete {
        ctx.file.sync_all().ok();
        Ok(())
    } else {
        Err(last_err.unwrap_or_else(|| anyhow!("téléchargement incomplet")))
    }
}

/// Une connexion : prend du travail, le télécharge, recommence.
async fn worker(ctx: Arc<Ctx>, delay: Duration) -> Result<()> {
    cancellable(&ctx.token, tokio::time::sleep(delay)).await?;
    loop {
        let Some(i) = claim(&ctx.item) else {
            return Ok(());
        };
        ctx.item.active_conns.fetch_add(1, Ordering::Relaxed);
        let mut attempt = 0;
        let res = loop {
            match fetch_segment(&ctx, i).await {
                Ok(()) => break Ok(()),
                Err(e) if ctx.token.is_cancelled() => break Err(e),
                Err(e) if e.downcast_ref::<Fatal>().is_some() => break Err(e),
                Err(e) => {
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        break Err(e);
                    }
                    let wait = Duration::from_millis(500 * 2u64.pow(attempt));
                    if cancellable(&ctx.token, tokio::time::sleep(wait)).await.is_err() {
                        break Err(e);
                    }
                }
            }
        };
        ctx.item.active_conns.fetch_sub(1, Ordering::Relaxed);
        ctx.item.rec.lock().unwrap().segments[i].active = false;
        res?;
    }
}

/// Choisit le prochain travail d'une connexion libre :
/// 1. un segment inactif non terminé (après une reprise), le plus grand d'abord ;
/// 2. sinon la moitié du plus grand segment en cours (règle d'IDM) ;
/// 3. sinon rien : la connexion s'arrête.
pub(crate) fn claim(item: &Item) -> Option<usize> {
    let mut r = item.rec.lock().unwrap();
    let segs = &mut r.segments;
    if let Some(i) = (0..segs.len())
        .filter(|&i| !segs[i].active && segs[i].remaining() > 0)
        .max_by_key(|&i| segs[i].remaining())
    {
        segs[i].active = true;
        return Some(i);
    }
    let i = (0..segs.len())
        .filter(|&i| segs[i].active)
        .max_by_key(|&i| segs[i].remaining())?;
    let rem = segs[i].remaining();
    if rem < 2 * MIN_SPLIT {
        return None;
    }
    let mid = segs[i].pos + rem / 2;
    let end = segs[i].end;
    segs[i].end = mid;
    segs.push(Segment { start: mid, end, pos: mid, active: true });
    Some(segs.len() - 1)
}

async fn fetch_segment(ctx: &Ctx, i: usize) -> Result<()> {
    let (from, to) = {
        let r = ctx.item.rec.lock().unwrap();
        (r.segments[i].pos, r.segments[i].end)
    };
    if from >= to {
        return Ok(());
    }
    let req = ctx
        .inner
        .client
        .get(&ctx.url)
        .headers(ctx.headers.clone())
        .header(header::RANGE, format!("bytes={}-{}", from, to - 1));
    let resp = cancellable(&ctx.token, req.send()).await?.context("connexion perdue")?;
    match resp.status() {
        StatusCode::PARTIAL_CONTENT => {}
        StatusCode::OK => bail!(Fatal("le serveur a ignoré la demande de plage".into())),
        s if s.is_client_error() && s != StatusCode::TOO_MANY_REQUESTS && s != StatusCode::REQUEST_TIMEOUT => {
            bail!(Fatal(format!("le serveur a répondu {}", s.as_u16())))
        }
        s => bail!("le serveur a répondu {}", s.as_u16()),
    }
    let mut stream = resp.bytes_stream();
    loop {
        let next = cancellable(&ctx.token, tokio::time::timeout(READ_TIMEOUT, stream.next())).await?;
        let chunk = match next {
            Err(_) => bail!("aucune donnée reçue depuis 30 s"),
            Ok(None) => break,
            Ok(Some(c)) => c.context("connexion interrompue")?,
        };
        // Réserve la place sous verrou : la fin du segment a pu être réduite
        // par une autre connexion venue le partager.
        let (off, n, done) = {
            let mut r = ctx.item.rec.lock().unwrap();
            let s = &mut r.segments[i];
            let n = (chunk.len() as u64).min(s.remaining());
            let off = s.pos;
            s.pos += n;
            (off, n, s.pos >= s.end)
        };
        if n > 0 {
            ctx.inner.limiter.acquire(n).await;
            if let Err(e) = write_at(&ctx.file, &chunk[..n as usize], off) {
                ctx.item.rec.lock().unwrap().segments[i].pos = off;
                return Err(anyhow!(Fatal(format!("écriture sur le disque impossible : {e}"))));
            }
        }
        if done {
            // On coupe : le reste appartient à une autre connexion.
            return Ok(());
        }
    }
    let r = ctx.item.rec.lock().unwrap();
    if r.segments[i].pos < r.segments[i].end {
        bail!("connexion fermée avant la fin du segment");
    }
    Ok(())
}

#[cfg(unix)]
fn write_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
fn write_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_with(segments: Vec<Segment>) -> Arc<Item> {
        crate::manager::test_item(segments)
    }

    #[test]
    fn claim_splits_largest_in_half() {
        let mb = MIN_SPLIT;
        let item = item_with(vec![Segment { start: 0, end: 100 * mb, pos: 0, active: false }]);
        assert_eq!(claim(&item), Some(0)); // première connexion : tout le fichier
        assert_eq!(claim(&item), Some(1)); // deuxième : la moitié
        {
            let r = item.rec.lock().unwrap();
            assert_eq!(r.segments[0].end, 50 * mb);
            assert_eq!(r.segments[1], Segment { start: 50 * mb, end: 100 * mb, pos: 50 * mb, active: true });
        }
        assert_eq!(claim(&item), Some(2)); // troisième : moitié du plus grand (le 0, à égalité)
        let r = item.rec.lock().unwrap();
        let mut ends: Vec<_> = r.segments.iter().map(|s| (s.start, s.end)).collect();
        ends.sort();
        // Les segments couvrent tout le fichier sans trou ni recouvrement.
        let mut cursor = 0;
        for (s, e) in ends {
            assert_eq!(s, cursor);
            cursor = e;
        }
        assert_eq!(cursor, 100 * mb);
    }

    #[test]
    fn claim_stops_when_too_small() {
        let item = item_with(vec![Segment { start: 0, end: MIN_SPLIT, pos: 0, active: true }]);
        assert_eq!(claim(&item), None);
    }

    #[test]
    fn claim_prefers_inactive_after_resume() {
        let mb = MIN_SPLIT;
        let item = item_with(vec![
            Segment { start: 0, end: 10 * mb, pos: 10 * mb, active: false },
            Segment { start: 10 * mb, end: 30 * mb, pos: 12 * mb, active: false },
        ]);
        assert_eq!(claim(&item), Some(1));
        assert_eq!(item.rec.lock().unwrap().segments[1].pos, 12 * mb);
    }
}
