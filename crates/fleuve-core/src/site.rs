//! Vidéos de sites (YouTube, Vimeo, Dailymotion…) : ces sites ne donnent pas
//! de lien direct vers un fichier, la vidéo est servie en flux séparés
//! (image et son). Fleuve confie l'extraction à `yt-dlp` (outil libre), puis
//! `ffmpeg` assemble image et son en un seul fichier MP4.
//!
//! Les vidéos protégées par DRM restent refusées (yt-dlp ne les contourne pas).

use crate::engine::Fatal;
use crate::manager::{Inner, Item};
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// Dossiers où chercher les outils : une app lancée depuis le Finder
/// n'hérite pas du PATH du terminal (Homebrew n'y figure pas).
const SEARCH_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

/// Commande conseillée pour installer yt-dlp et ffmpeg selon le système.
pub const INSTALL_HINT: &str = if cfg!(target_os = "macos") {
    "dans le Terminal : brew install yt-dlp ffmpeg"
} else if cfg!(target_os = "windows") {
    "dans un terminal : winget install yt-dlp.yt-dlp Gyan.FFmpeg"
} else {
    "dans un terminal : pipx install yt-dlp (ou pip install --user yt-dlp), puis installez ffmpeg avec le gestionnaire de paquets"
};

pub fn find_tool(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(SEARCH_DIRS.iter().map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".local/bin"));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Sites dont les pages vidéo sont confiées à yt-dlp.
pub fn is_video_site(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.");
    let path = u.path();
    match host {
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => {
            path.starts_with("/watch")
                || path.starts_with("/shorts/")
                || path.starts_with("/embed/")
                || path.starts_with("/live/")
        }
        "youtu.be" => path.len() > 1,
        "vimeo.com" | "player.vimeo.com" | "dailymotion.com" | "dai.ly" | "twitter.com" | "x.com"
        | "facebook.com" | "fb.watch" | "instagram.com" | "tiktok.com" | "twitch.tv"
        | "clips.twitch.tv" | "soundcloud.com" | "bilibili.com" | "reddit.com" => true,
        _ => false,
    }
}

/// Ligne de progression émise par yt-dlp grâce à `--progress-template`.
const MARK: &str = "FLEUVE:";

pub(crate) async fn run_site(inner: &Arc<Inner>, item: &Arc<Item>, token: &CancellationToken) -> Result<()> {
    let ytdlp = find_tool("yt-dlp").ok_or_else(|| {
        anyhow!(Fatal(format!("yt-dlp est introuvable. Installez-le {INSTALL_HINT}")))
    })?;
    let ffmpeg = find_tool("ffmpeg");
    let (url, cookie) = {
        let r = item.rec.lock().unwrap();
        (r.url.clone(), r.headers.cookie.clone().filter(|c| !c.trim().is_empty()))
    };

    // 1er essai sans session : c'est ce qui donne les meilleurs formats sur
    // YouTube. Si le site exige d'être connecté, on réessaie avec les cookies
    // du navigateur (transmis par l'extension, jamais envoyés ailleurs).
    // YouTube refuse parfois un lien de flux (erreur 403) : relancer yt-dlp
    // obtient des liens neufs et reprend là où on en était.
    let mut msg = String::new();
    for essai in 0..3 {
        if essai > 0 {
            tokio::select! {
                _ = token.cancelled() => bail!("annulé"),
                _ = tokio::time::sleep(std::time::Duration::from_secs(2 * essai)) => {}
            }
        }
        match attempt(inner, item, token, &ytdlp, ffmpeg.as_deref(), None).await? {
            Ok(()) => return Ok(()),
            Err(m) => msg = m,
        }
        if !is_transient(&msg) {
            break;
        }
    }
    let Some(cookie) = cookie.filter(|_| needs_login(&msg)) else {
        bail!(Fatal(explain(&msg)));
    };
    let jar = write_cookie_jar(&url, &cookie)?;
    let second = attempt(inner, item, token, &ytdlp, ffmpeg.as_deref(), Some(&jar)).await;
    let _ = std::fs::remove_file(&jar);
    match second? {
        Ok(()) => Ok(()),
        Err(msg) => bail!(Fatal(explain(&msg))),
    }
}

/// Supprime les morceaux laissés par yt-dlp pour une vidéo non terminée
/// (`Titre [id].f137.mp4`, `Titre [id].f140.m4a.part`, `.ytdl`…).
pub(crate) fn remove_partials(dir: &Path, file_name: &str) {
    let Some(stem) = file_name.strip_suffix(".mp4") else { return };
    if !stem.ends_with(']') {
        return; // nom provisoire : yt-dlp n'a encore rien écrit
    }
    let prefix = format!("{stem}.");
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name != file_name {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Erreur passagère qui vaut une nouvelle tentative.
fn is_transient(msg: &str) -> bool {
    let l = msg.to_lowercase();
    ["403", "http error 5", "timed out", "connection reset", "incomplete", "temporary", "unable to download video data"]
        .iter()
        .any(|k| l.contains(k))
}

/// Le message d'erreur indique-t-il qu'une session connectée est nécessaire ?
fn needs_login(msg: &str) -> bool {
    let l = msg.to_lowercase();
    [
        "sign in", "log in", "login", "logged-in", "cookies", "your age", "age-restricted", "private", "registered users",
        "authentication", "members", "subscriber", "account", "rate-limit", "not a bot",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Écrit les cookies au format Netscape (lu par yt-dlp), lisible par l'utilisateur seul.
fn write_cookie_jar(url: &str, cookie: &str) -> Result<PathBuf> {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let mut domain = host.as_str();
    for p in ["www.", "m.", "music.", "mobile.", "web."] {
        domain = domain.strip_prefix(p).unwrap_or(domain);
    }
    let expires = crate::util::now_secs() + 86_400;
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for pair in cookie.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else { continue };
        if name.is_empty() || name.contains(['\t', '\n']) || value.contains(['\t', '\n']) {
            continue;
        }
        out.push_str(&format!(".{domain}\tTRUE\t/\tTRUE\t{expires}\t{name}\t{value}\n"));
    }
    let path = std::env::temp_dir().join(format!("fleuve-cookies-{}.txt", uuid::Uuid::new_v4().simple()));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&path)?.write_all(out.as_bytes())?;
    }
    Ok(path)
}

/// Une exécution de yt-dlp. `Ok(Err(message))` : yt-dlp a échoué (message brut).
async fn attempt(
    inner: &Arc<Inner>,
    item: &Arc<Item>,
    token: &CancellationToken,
    ytdlp: &Path,
    ffmpeg: Option<&Path>,
    cookie_jar: Option<&Path>,
) -> Result<std::result::Result<(), String>> {
    let (url, dir, user_agent) = {
        let r = item.rec.lock().unwrap();
        (r.url.clone(), r.dest_dir.clone(), r.headers.user_agent.clone())
    };
    std::fs::create_dir_all(&dir)?;
    let connections = inner.settings().connections.clamp(1, 16);
    let limit = inner.settings().speed_limit_kbps;

    let mut cmd = Command::new(ytdlp);
    cmd.arg("--newline")
        .arg("--no-playlist")
        // Une seule vidéo par demande : une chaîne, une liste ou un direct
        // sont refusés au lieu de télécharger des dizaines de vidéos.
        .args(["--playlist-items", "1"])
        .args(["--match-filters", "!playlist_id & !is_live"])
        .arg("--no-mtime")
        .arg("--no-colors")
        .arg("--continue")
        .args(["--concurrent-fragments", &connections.to_string()])
        .args(["-P", &dir.to_string_lossy()])
        .args(["-o", "%(title).180B [%(id)s].%(ext)s"])
        .args([
            "--progress-template",
            "download:FLEUVE:%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s",
        ])
        .args(["--print", "before_dl:FLEUVE-NAME:%(title).180B [%(id)s]"])
        .args(["--print", "after_move:FLEUVE-FILE:%(filepath)s"])
        .arg("--no-simulate")
        .arg("--no-quiet") // garde les messages « skipping » pour expliquer un refus
        .arg("--progress");
    if let Some(jar) = cookie_jar {
        cmd.arg("--cookies").arg(jar);
    }
    if let Some(ff) = ffmpeg {
        // Meilleure image + meilleur son, assemblés en MP4 (lisible partout sur Mac).
        cmd.args(["--ffmpeg-location", &ff.to_string_lossy()])
            .args(["-f", "bv*[vcodec^=avc1]+ba[acodec^=mp4a]/bv*+ba/b"])
            .args(["--merge-output-format", "mp4"]);
    } else {
        // Sans ffmpeg : un seul fichier déjà assemblé (qualité plus basse).
        cmd.args(["-f", "b"]);
    }
    if limit > 0 {
        cmd.args(["--limit-rate", &format!("{limit}K")]);
    }
    if let Some(ua) = user_agent.filter(|u| !u.is_empty()) {
        cmd.args(["--user-agent", &ua]);
    }
    cmd.arg("--").arg(&url);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| anyhow!(Fatal(format!("lancement de yt-dlp impossible : {e}"))))?;
    let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    item.active_conns.store(connections, Ordering::Relaxed);
    {
        let mut r = item.rec.lock().unwrap();
        r.downloaded = 0;
        r.total = None;
        r.category = "Vidéos".into();
    }

    let mut lines = merge(stdout, stderr);
    let mut base: u64 = 0; // octets des flux déjà terminés (image puis son)
    let mut last: u64 = 0;
    let mut last_error: Option<String> = None;
    let mut final_file: Option<PathBuf> = None;
    let mut skipped: Option<&'static str> = None;
    let mut saw_playlist = false;

    loop {
        let line = tokio::select! {
            _ = token.cancelled() => {
                let _ = child.kill().await;
                bail!("annulé");
            }
            l = lines.recv() => l,
        };
        let Some(line) = line else { break };
        let line = line.trim();
        if let Some(p) = line.strip_prefix(MARK) {
            let mut it = p.split('|').map(|v| v.trim().parse::<f64>().ok().map(|f| f as u64));
            let done = it.next().flatten().unwrap_or(0);
            let total = it.next().flatten().or(it.next().flatten());
            if done < last {
                base += last; // nouveau flux
            }
            last = done;
            let mut r = item.rec.lock().unwrap();
            r.downloaded = base + done;
            r.total = total.map(|t| base + t.max(done));
        } else if let Some(n) = line.strip_prefix("FLEUVE-NAME:") {
            let mut r = item.rec.lock().unwrap();
            r.file_name = format!("{n}.mp4");
            r.name_fixed = true;
        } else if let Some(f) = line.strip_prefix("FLEUVE-FILE:") {
            final_file = Some(PathBuf::from(f));
        } else if line.contains("does not pass filter") {
            // Testé avant la détection de liste : ce message contient « playlist_id ».
            skipped = Some(if !saw_playlist {
                "c'est une vidéo en direct : Fleuve ne télécharge que des vidéos terminées"
            } else {
                "ce lien mène à une chaîne ou une liste de vidéos, pas à une vidéo. Ouvrez la vidéo voulue puis réessayez"
            });
        } else if line.starts_with("ERROR:") {
            last_error = Some(line.trim_start_matches("ERROR:").trim().to_string());
        } else if line.contains(":tab]")
            || line.contains("uploads of the channel")
            || line.to_lowercase().contains("playlist")
        {
            saw_playlist = true;
        }
    }
    let status = child.wait().await?;
    if token.is_cancelled() {
        bail!("annulé");
    }
    if let Some(why) = skipped.filter(|_| final_file.is_none()) {
        bail!(Fatal(why.into()));
    }
    if status.success() && final_file.is_none() {
        bail!(Fatal("aucune vidéo trouvée à cette adresse".into()));
    }
    if !status.success() {
        return Ok(Err(last_error.unwrap_or_else(|| format!("yt-dlp a échoué ({status})"))));
    }
    let mut r = item.rec.lock().unwrap();
    if let Some(f) = final_file {
        if let Some(name) = f.file_name() {
            r.file_name = name.to_string_lossy().into_owned();
        }
        if let Some(parent) = f.parent() {
            r.dest_dir = parent.to_path_buf();
        }
        if let Ok(m) = std::fs::metadata(&f) {
            r.total = Some(m.len());
            r.downloaded = m.len();
        }
    }
    Ok(Ok(()))
}

/// Messages de yt-dlp traduits quand on les reconnaît.
fn explain(msg: &str) -> String {
    let l = msg.to_lowercase();
    if l.contains("drm") {
        "vidéo protégée par DRM : Fleuve ne contourne pas les protections".into()
    } else if l.contains("sign in to confirm your age") || l.contains("age-restricted") {
        "vidéo réservée aux adultes : connexion au compte requise".into()
    } else if l.contains("private video") {
        "vidéo privée".into()
    } else if l.contains("unsupported url") {
        "aucune vidéo trouvée à cette adresse".into()
    } else {
        format!("yt-dlp : {msg}")
    }
}

/// Fusionne la sortie standard et la sortie d'erreur en un seul flux de lignes.
fn merge<A, B>(
    mut a: tokio::io::Lines<BufReader<A>>,
    mut b: tokio::io::Lines<BufReader<B>>,
) -> tokio::sync::mpsc::UnboundedReceiver<String>
where
    A: tokio::io::AsyncRead + Unpin + Send + 'static,
    B: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx2 = tx.clone();
    tokio::spawn(async move {
        while let Ok(Some(l)) = a.next_line().await {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Ok(Some(l)) = b.next_line().await {
            if tx2.send(l).is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_sites() {
        assert!(is_video_site("https://www.youtube.com/watch?v=WSL8730oQ8A"));
        assert!(is_video_site("https://youtu.be/WSL8730oQ8A"));
        assert!(is_video_site("https://m.youtube.com/shorts/abc"));
        assert!(!is_video_site("https://www.youtube.com/"));
        assert!(!is_video_site("https://example.com/video.mp4"));
    }

    #[test]
    fn cookie_jar_format() {
        let p = write_cookie_jar("https://www.youtube.com/watch?v=x", "SID=abc; HSID=d=e; bad").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        let lines: Vec<_> = text.lines().skip(1).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(".youtube.com\tTRUE\t/\tTRUE\t"));
        assert!(lines[0].ends_with("\tSID\tabc"));
        assert!(lines[1].ends_with("\tHSID\td=e"));
    }

    #[test]
    fn partials_removed() {
        let d = tempfile::tempdir().unwrap();
        for n in ["Clip [abc].f137.mp4", "Clip [abc].f140.m4a.part", "Clip [abc].mp4.ytdl", "Autre [xyz].mp4"] {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        remove_partials(d.path(), "Clip [abc].mp4");
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("Autre [xyz].mp4")]);
    }

    #[test]
    fn transient_errors() {
        assert!(is_transient("unable to download video data: HTTP Error 403: Forbidden"));
        assert!(!is_transient("Private video"));
    }

    #[test]
    fn login_errors() {
        assert!(needs_login("Sign in to confirm your age"));
        assert!(needs_login("This video is private"));
        assert!(!needs_login("Unsupported URL: https://example.com"));
    }
}
