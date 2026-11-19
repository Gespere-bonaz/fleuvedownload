//! Petites fonctions utilitaires : noms de fichiers, catégories, horloge.

use percent_encoding::percent_decode_str;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Catégorie IDM-like selon l'extension du fichier.
pub fn category_for(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "ts" | "flv" | "wmv" | "m3u8" => "Vidéos",
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "opus" | "wma" => "Musique",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "txt" | "epub" | "csv" => {
            "Documents"
        }
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "tgz" => "Archives",
        "dmg" | "pkg" | "app" | "exe" | "msi" | "deb" | "rpm" | "apk" | "iso" | "appimage" => {
            "Programmes"
        }
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "heic" => "Images",
        _ => "Autres",
    }
}

/// Retire les caractères interdits dans un nom de fichier macOS/Windows.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        "telechargement".to_string()
    } else {
        // Limite raisonnable pour les systèmes de fichiers (255 octets).
        let mut out = String::new();
        for c in trimmed.chars() {
            if out.len() + c.len_utf8() > 200 {
                break;
            }
            out.push(c);
        }
        out
    }
}

/// Nom de fichier déduit du dernier élément du chemin de l'URL.
pub fn file_name_from_url(url: &str) -> String {
    let name = url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(|s| s.to_string()))
        })
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode_str(&s).decode_utf8_lossy().into_owned())
        .unwrap_or_else(|| "telechargement".to_string());
    sanitize_file_name(&name)
}

/// Extrait le nom de fichier d'un en-tête `Content-Disposition`.
/// Gère `filename*=UTF-8''...` (RFC 5987) et `filename="..."`.
pub fn file_name_from_disposition(value: &str) -> Option<String> {
    let mut plain: Option<String> = None;
    for part in value.split(';') {
        let part = part.trim();
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename*=") {
            let v = &part["filename*=".len()..];
            let v = v.trim_matches('"');
            // charset'lang'valeur
            let encoded = v.splitn(3, '\'').nth(2).unwrap_or(v);
            let decoded = percent_decode_str(encoded).decode_utf8_lossy().into_owned();
            if !decoded.is_empty() {
                return Some(sanitize_file_name(&decoded));
            }
        } else if lower.starts_with("filename=") {
            let v = part["filename=".len()..].trim().trim_matches('"');
            if !v.is_empty() {
                plain = Some(sanitize_file_name(v));
            }
        }
    }
    plain
}

/// Chemin du fichier temporaire pendant le téléchargement.
pub fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.fleuvepart"))
}

/// Trouve un nom libre : « fichier.zip », puis « fichier (1).zip », etc.
/// Un nom est pris si le fichier final ou son fichier temporaire existe.
pub fn unique_name(dir: &Path, name: &str) -> String {
    let taken = |n: &str| dir.join(n).exists() || part_path(dir, n).exists();
    if !taken(name) {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|e| e.to_str());
    for i in 1..10_000 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{}-{}", uuid::Uuid::new_v4().simple(), name)
}

/// Remplace l'extension d'un nom de fichier.
pub fn with_extension(name: &str, ext: &str) -> String {
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    format!("{stem}.{ext}")
}

pub fn looks_like_hls(url: &str) -> bool {
    url::Url::parse(url)
        .map(|u| u.path().to_ascii_lowercase().ends_with(".m3u8"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition() {
        assert_eq!(
            file_name_from_disposition("attachment; filename=\"rapport final.pdf\"").as_deref(),
            Some("rapport final.pdf")
        );
        assert_eq!(
            file_name_from_disposition("attachment; filename=a.pdf; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf")
                .as_deref(),
            Some("résumé.pdf")
        );
        assert_eq!(file_name_from_disposition("inline"), None);
    }

    #[test]
    fn url_names() {
        assert_eq!(file_name_from_url("https://x.org/a/b/mon%20fichier.zip?x=1"), "mon fichier.zip");
        assert_eq!(file_name_from_url("https://x.org/"), "telechargement");
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
    }

    #[test]
    fn categories() {
        assert_eq!(category_for("film.MKV"), "Vidéos");
        assert_eq!(category_for("setup.dmg"), "Programmes");
        assert_eq!(category_for("inconnu"), "Autres");
    }
}
