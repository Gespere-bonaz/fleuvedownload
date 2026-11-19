// Réglages et communication avec l'application Fleuve (127.0.0.1 uniquement).

export const DEFAULTS = {
  token: "",
  port: 47821,
  intercept: true,
  minSizeMb: 1,
  extensions:
    "zip rar 7z tar gz bz2 xz dmg pkg iso exe msi deb rpm apk mp4 mkv mov avi webm m4v mp3 flac wav m4a aac pdf epub",
};

export async function getSettings() {
  const s = await chrome.storage.local.get(DEFAULTS);
  return { ...DEFAULTS, ...s };
}

/** Cookies du site pour reproduire une session connectée (envoyés seulement à Fleuve, en local). */
async function cookieHeader(url) {
  try {
    const list = await chrome.cookies.getAll({ url });
    return list.map((c) => `${c.name}=${c.value}`).join("; ") || null;
  } catch {
    return null;
  }
}

async function call(path, init = {}) {
  const s = await getSettings();
  if (!s.token) throw new Error("Jeton manquant : ouvrez les options de l'extension.");
  const res = await fetch(`http://127.0.0.1:${s.port}${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", "X-Fleuve-Token": s.token, ...(init.headers || {}) },
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok || !body.ok) throw new Error(body.error || `Erreur ${res.status}`);
  return body;
}

export async function ping() {
  return call("/ping");
}

/** Envoie un lien à Fleuve. `kind` : "hls" pour une liste de lecture vidéo. */
export async function sendToFleuve(
  url,
  { referer = null, fileName = null, kind = null, focus = true, startPaused = false } = {},
) {
  return call("/add", {
    method: "POST",
    body: JSON.stringify({
      url,
      referer,
      fileName,
      kind,
      focus,
      startPaused,
      cookie: await cookieHeader(url),
      userAgent: navigator.userAgent,
    }),
  });
}

/** Pages vidéo confiées au moteur yt-dlp de Fleuve (pas de lien direct au fichier). */
const VIDEO_SITES =
  /(^|\.)(youtube\.com|youtube-nocookie\.com|youtu\.be|vimeo\.com|dailymotion\.com|dai\.ly|twitter\.com|x\.com|facebook\.com|fb\.watch|instagram\.com|tiktok\.com|twitch\.tv|soundcloud\.com|bilibili\.com|reddit\.com)$/i;

export function isVideoSite(url) {
  try {
    return VIDEO_SITES.test(new URL(url).hostname);
  } catch {
    return false;
  }
}

/** Fragments de flux (YouTube, etc.) : inutiles seuls, on ne les propose pas. */
export function isStreamFragment(url) {
  try {
    const u = new URL(url);
    return /(^|\.)googlevideo\.com$/i.test(u.hostname) || u.searchParams.has("range") || /\/range\/\d/.test(u.pathname);
  } catch {
    return false;
  }
}

export function extOf(url) {
  try {
    const p = new URL(url).pathname.toLowerCase();
    const i = p.lastIndexOf(".");
    return i >= 0 ? p.slice(i + 1) : "";
  } catch {
    return "";
  }
}
