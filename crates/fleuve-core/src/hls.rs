//! Capture vidéo HLS (.m3u8) : lecture de la liste de lecture, choix de la
//! meilleure qualité, téléchargement parallèle des morceaux, assemblage.
//!
//! Seuls les flux **non chiffrés** et **terminés** (VOD) sont pris en charge.
//! Fleuve ne contourne aucune protection (DRM, chiffrement).

use crate::engine::{cancellable, request_headers, settle_name, Fatal};
use crate::manager::{Inner, Item};
use crate::model::*;
use crate::util;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Debug, PartialEq)]
pub struct Variant {
    pub bandwidth: u64,
    /// Hauteur d'image tirée de RESOLUTION (720 pour 1280x720), si indiquée.
    pub height: Option<u32>,
    pub url: Url,
}

/// Choisit la variante : le meilleur débit parmi celles qui ne dépassent pas
/// `max_height` ; si aucune ne convient, la plus basse résolution connue.
pub fn pick_variant(variants: &[Variant], max_height: Option<u32>) -> Option<&Variant> {
    let Some(max) = max_height else {
        return variants.iter().max_by_key(|v| v.bandwidth);
    };
    variants
        .iter()
        .filter(|v| v.height.map_or(false, |h| h <= max))
        .max_by_key(|v| (v.height, v.bandwidth))
        .or_else(|| variants.iter().filter(|v| v.height.is_some()).min_by_key(|v| (v.height, v.bandwidth)))
        .or_else(|| variants.iter().max_by_key(|v| v.bandwidth))
}

#[derive(Debug, PartialEq)]
pub struct Media {
    /// Segment d'initialisation (fMP4), à placer en tête.
    pub init: Option<Url>,
    pub segments: Vec<Url>,
    pub encrypted: bool,
    pub ended: bool,
    pub byte_ranges: bool,
}

#[derive(Debug, PartialEq)]
pub enum Playlist {
    Master(Vec<Variant>),
    Media(Media),
}

fn attr<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    // Attributs « CLE=valeur » séparés par des virgules, valeurs parfois entre guillemets.
    let attrs = line.split_once(':')?.1;
    let mut rest = attrs;
    while !rest.is_empty() {
        let (k, after) = rest.split_once('=')?;
        let (v, next) = if let Some(stripped) = after.strip_prefix('"') {
            let end = stripped.find('"')?;
            (&stripped[..end], stripped[end + 1..].trim_start_matches(','))
        } else {
            match after.split_once(',') {
                Some((v, n)) => (v, n),
                None => (after, ""),
            }
        };
        if k.trim().eq_ignore_ascii_case(key) {
            return Some(v);
        }
        rest = next;
    }
    None
}

pub fn parse_playlist(text: &str, base: &Url) -> Result<Playlist> {
    let text = text.trim_start_matches('\u{feff}');
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!(Fatal("ce n'est pas une liste de lecture HLS".into()));
    }
    let mut variants = Vec::new();
    let mut pending_bw: Option<(u64, Option<u32>)> = None;
    let mut media = Media { init: None, segments: vec![], encrypted: false, ended: false, byte_ranges: false };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(tag) = line.strip_prefix('#') {
            if tag.starts_with("EXT-X-STREAM-INF") {
                let bw = attr(line, "AVERAGE-BANDWIDTH")
                    .or_else(|| attr(line, "BANDWIDTH"))
                    .and_then(|b| b.parse().ok())
                    .unwrap_or(0);
                let height = attr(line, "RESOLUTION").and_then(|r| r.split_once(['x', 'X'])).and_then(|(_, h)| h.parse().ok());
                pending_bw = Some((bw, height));
            } else if tag.starts_with("EXT-X-KEY") {
                let method = attr(line, "METHOD").unwrap_or("NONE");
                if !method.eq_ignore_ascii_case("NONE") {
                    media.encrypted = true;
                }
            } else if tag.starts_with("EXT-X-MAP") {
                if let Some(uri) = attr(line, "URI") {
                    media.init = Some(base.join(uri)?);
                }
            } else if tag.starts_with("EXT-X-ENDLIST") {
                media.ended = true;
            } else if tag.starts_with("EXT-X-BYTERANGE") {
                media.byte_ranges = true;
            }
            continue;
        }
        let url = base.join(line).with_context(|| format!("adresse invalide : {line}"))?;
        if let Some((bw, height)) = pending_bw.take() {
            variants.push(Variant { bandwidth: bw, height, url });
        } else {
            media.segments.push(url);
        }
    }
    if !variants.is_empty() {
        Ok(Playlist::Master(variants))
    } else {
        Ok(Playlist::Media(media))
    }
}

pub(crate) fn temp_dir(dest: &Path, name: &str) -> PathBuf {
    dest.join(format!(".{name}.fleuve-hls"))
}

async fn fetch_text(inner: &Inner, url: &Url, headers: &HeaderMap) -> Result<String> {
    let resp = inner.client.get(url.clone()).headers(headers.clone()).send().await?;
    if !resp.status().is_success() {
        bail!(Fatal(format!("liste de lecture : le serveur a répondu {}", resp.status().as_u16())));
    }
    Ok(resp.text().await?)
}

/// Qualités proposées par une liste maître HLS (vide si une seule qualité).
pub(crate) async fn probe_qualities(inner: &Inner, url: &str, headers: &RequestHeaders) -> Result<Vec<QualityOption>> {
    let url = Url::parse(url)?;
    let text = fetch_text(inner, &url, &request_headers(headers)).await?;
    let Playlist::Master(variants) = parse_playlist(&text, &url)? else {
        return Ok(vec![]);
    };
    let mut heights: Vec<u32> = variants.iter().filter_map(|v| v.height).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    Ok(heights.into_iter().map(|height| QualityOption { height, size: None }).collect())
}

/// Lit la liste maître si besoin et renvoie la liste des morceaux de la qualité choisie
/// (la meilleure sans limite, sinon la meilleure sous `max_height`).
async fn resolve_media(inner: &Inner, url: &Url, headers: &HeaderMap, max_height: Option<u32>) -> Result<Media> {
    let mut current = url.clone();
    for _ in 0..3 {
        let text = fetch_text(inner, &current, headers).await?;
        match parse_playlist(&text, &current)? {
            Playlist::Media(m) => return Ok(m),
            Playlist::Master(vs) => {
                current = pick_variant(&vs, max_height)
                    .map(|v| v.url.clone())
                    .ok_or_else(|| anyhow!("liste maître vide"))?;
            }
        }
    }
    bail!(Fatal("trop de niveaux de listes de lecture".into()))
}

pub(crate) async fn run_hls(inner: &Arc<Inner>, item: &Arc<Item>, token: &CancellationToken) -> Result<()> {
    let (url, headers, max_height) = {
        let r = item.rec.lock().unwrap();
        (Url::parse(&r.url)?, request_headers(&r.headers), r.max_height)
    };
    let media = cancellable(token, resolve_media(inner, &url, &headers, max_height)).await??;
    if media.encrypted {
        bail!(Fatal(
            "flux chiffré : Fleuve ne télécharge que les flux HLS non protégés".into()
        ));
    }
    if !media.ended {
        bail!(Fatal("flux en direct : non pris en charge dans cette version".into()));
    }
    if media.byte_ranges {
        bail!(Fatal("liste de lecture à plages d'octets : non prise en charge".into()));
    }
    let fmp4 = media.init.is_some()
        || media.segments.first().map_or(false, |u| {
            let p = u.path().to_ascii_lowercase();
            p.ends_with(".m4s") || p.ends_with(".mp4")
        });
    let parts: Vec<Url> = media.init.into_iter().chain(media.segments).collect();
    if parts.is_empty() {
        bail!(Fatal("liste de lecture sans morceaux".into()));
    }
    {
        let mut r = item.rec.lock().unwrap();
        if !r.name_fixed {
            r.file_name = util::with_extension(&r.file_name, if fmp4 { "mp4" } else { "ts" });
            r.category = "Vidéos".into();
        }
    }
    let (dir, name) = settle_name(item, None)?;
    let tmp = temp_dir(&dir, &name);
    tokio::fs::create_dir_all(&tmp).await?;

    let done: HashSet<u32> = {
        let mut r = item.rec.lock().unwrap();
        r.resumable = true;
        r.segments.clear();
        if r.hls_total_parts as usize != parts.len() {
            r.hls_total_parts = parts.len() as u32;
            r.hls_done.clear();
            r.downloaded = 0;
        }
        // Un morceau noté terminé doit encore exister sur le disque.
        let tmp2 = tmp.clone();
        r.hls_done.retain(|&i| part_file(&tmp2, i).exists());
        r.downloaded = r
            .hls_done
            .iter()
            .filter_map(|&i| std::fs::metadata(part_file(&tmp2, i)).ok())
            .map(|m| m.len())
            .sum();
        r.hls_done.iter().copied().collect()
    };

    let work = token.child_token();
    let sem = Arc::new(Semaphore::new(inner.settings().connections.clamp(1, 16) as usize));
    let mut set = JoinSet::new();
    for (i, u) in parts.iter().enumerate() {
        let i = i as u32;
        if done.contains(&i) {
            continue;
        }
        let (inner, item, work, sem, headers, tmp, u) =
            (inner.clone(), item.clone(), work.clone(), sem.clone(), headers.clone(), tmp.clone(), u.clone());
        set.spawn(async move {
            let _permit = cancellable(&work, sem.acquire_owned()).await??;
            item.active_conns.fetch_add(1, Ordering::Relaxed);
            let res = fetch_part(&inner, &item, &work, &u, &headers, &tmp, i).await;
            item.active_conns.fetch_sub(1, Ordering::Relaxed);
            res
        });
    }
    let mut first_err = None;
    while let Some(res) = set.join_next().await {
        let res = res.map_err(|e| anyhow!("tâche interrompue : {e}")).and_then(|r| r);
        if let Err(e) = res {
            if first_err.is_none() {
                work.cancel(); // un morceau perdu = vidéo inutilisable
                first_err = Some(e);
            }
        }
    }
    if token.is_cancelled() {
        bail!("annulé");
    }
    if let Some(e) = first_err {
        return Err(e);
    }

    // Assemblage dans l'ordre.
    item.rec.lock().unwrap().status = Status::Assembling;
    let part = util::part_path(&dir, &name);
    let mut out = tokio::fs::File::create(&part).await?;
    for i in 0..parts.len() as u32 {
        let mut f = tokio::fs::File::open(part_file(&tmp, i))
            .await
            .with_context(|| format!("morceau {i} manquant"))?;
        tokio::io::copy(&mut f, &mut out).await?;
        if token.is_cancelled() {
            bail!("annulé");
        }
    }
    out.flush().await?;
    drop(out);
    tokio::fs::rename(&part, dir.join(&name)).await?;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    let mut r = item.rec.lock().unwrap();
    r.total = Some(r.downloaded);
    Ok(())
}

fn part_file(tmp: &Path, i: u32) -> PathBuf {
    tmp.join(format!("{i:06}.seg"))
}

async fn fetch_part(
    inner: &Inner,
    item: &Item,
    token: &CancellationToken,
    url: &Url,
    headers: &HeaderMap,
    tmp: &Path,
    i: u32,
) -> Result<()> {
    let mut attempt = 0;
    loop {
        match fetch_part_once(inner, item, token, url, headers, tmp, i).await {
            Ok(()) => return Ok(()),
            Err(e) if token.is_cancelled() || e.downcast_ref::<Fatal>().is_some() => return Err(e),
            Err(e) => {
                attempt += 1;
                if attempt >= 4 {
                    return Err(e.context(format!("morceau {i}")));
                }
                cancellable(token, tokio::time::sleep(Duration::from_millis(700 * attempt))).await?;
            }
        }
    }
}

async fn fetch_part_once(
    inner: &Inner,
    item: &Item,
    token: &CancellationToken,
    url: &Url,
    headers: &HeaderMap,
    tmp: &Path,
    i: u32,
) -> Result<()> {
    let resp = cancellable(token, inner.client.get(url.clone()).headers(headers.clone()).send()).await??;
    let status = resp.status();
    if status.is_client_error() && status.as_u16() != 429 && status.as_u16() != 408 {
        bail!(Fatal(format!("morceau {i} : le serveur a répondu {}", status.as_u16())));
    }
    if !status.is_success() {
        bail!("morceau {i} : le serveur a répondu {}", status.as_u16());
    }
    let staging = tmp.join(format!("{i:06}.tmp"));
    let mut file = tokio::fs::File::create(&staging).await?;
    let mut stream = resp.bytes_stream();
    let mut got: u64 = 0;
    while let Some(chunk) = cancellable(token, tokio::time::timeout(Duration::from_secs(30), stream.next()))
        .await?
        .map_err(|_| anyhow!("aucune donnée reçue depuis 30 s"))?
    {
        let chunk = chunk?;
        inner.limiter.acquire(chunk.len() as u64).await;
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;
        item.rec.lock().unwrap().downloaded += chunk.len() as u64;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&staging, part_file(tmp, i)).await.map_err(|e| {
        item.rec.lock().unwrap().downloaded -= got;
        e
    })?;
    item.rec.lock().unwrap().hls_done.push(i);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_and_media() {
        let base = Url::parse("https://cdn.example.org/v/master.m3u8").unwrap();
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2400000,RESOLUTION=1280x720\nhigh/index.m3u8\n";
        match parse_playlist(master, &base).unwrap() {
            Playlist::Master(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[1].bandwidth, 2_400_000);
                assert_eq!(v[1].url.as_str(), "https://cdn.example.org/v/high/index.m3u8");
            }
            _ => panic!("liste maître attendue"),
        }
        let media = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\nseg0.m4s\n#EXTINF:6.0,\n/abs/seg1.m4s?t=1\n#EXT-X-ENDLIST\n";
        match parse_playlist(media, &base).unwrap() {
            Playlist::Media(m) => {
                assert!(m.ended && !m.encrypted);
                assert_eq!(m.init.unwrap().as_str(), "https://cdn.example.org/v/init.mp4");
                assert_eq!(m.segments[1].as_str(), "https://cdn.example.org/abs/seg1.m4s?t=1");
            }
            _ => panic!("liste média attendue"),
        }
    }

    #[test]
    fn picks_variant_by_quality() {
        let base = Url::parse("https://cdn.example.org/v/master.m3u8").unwrap();
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2400000,RESOLUTION=1280x720\nmid.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=6000000,RESOLUTION=1920x1080\nhigh.m3u8\n";
        let Playlist::Master(v) = parse_playlist(master, &base).unwrap() else { panic!("liste maître attendue") };
        assert_eq!(v[2].height, Some(1080));
        let name = |h| pick_variant(&v, h).unwrap().url.path().rsplit('/').next().unwrap().to_string();
        assert_eq!(name(None), "high.m3u8");
        assert_eq!(name(Some(720)), "mid.m3u8");
        assert_eq!(name(Some(1000)), "mid.m3u8");
        assert_eq!(name(Some(240)), "low.m3u8"); // rien en dessous : la plus basse
    }

    #[test]
    fn detects_encryption() {
        let base = Url::parse("https://x.org/a.m3u8").unwrap();
        let t = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:4,\na.ts\n#EXT-X-ENDLIST";
        let Playlist::Media(m) = parse_playlist(t, &base).unwrap() else { panic!() };
        assert!(m.encrypted);
        let t = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\na.ts\n";
        let Playlist::Media(m) = parse_playlist(t, &base).unwrap() else { panic!() };
        assert!(!m.encrypted && !m.ended);
    }
}
