// Réglages et communication avec l'application Fleuve (127.0.0.1 uniquement).

export const DEFAULTS = {
  token: "",
  port: 47821,
  intercept: true,
  minSizeMb: 1,
  // Qualité des vidéos envoyées par l'extension : "default" (réglage de Fleuve),
  // "best", "ask" (choisir dans Fleuve) ou une hauteur maximale ("1080", "720"…).
  quality: "default",
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

/** Appairage : routes sans jeton (c'est leur but) ; Fleuve demande l'accord de l'utilisateur. */
async function pairCall(port, route, body) {
  const res = await fetch(`http://127.0.0.1:${port}${route}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body ?? {}),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok || !data.ok) throw new Error(data.error || `Erreur ${res.status}`);
  return data;
}

/** Demande un appairage : { id, code, expiresIn }. Le code s'affiche aussi dans Fleuve. */
export const pairStart = (port) => pairCall(port, "/pair/start");

/** État de la demande : { state: "pending" | "approved" | "denied" | "expired", token? }. */
export const pairStatus = (port, id) => pairCall(port, "/pair/status", { id });

/**
 * Envoie un lien à Fleuve. `kind` : "hls" pour une liste de lecture vidéo.
 * `title` : titre de la vidéo, devenu nom du fichier (l'extension est choisie par Fleuve).
 * `quality` : "best", "ask" ou une hauteur ("720"). Non précisé : réglage des options
 * de l'extension ; « default » laisse Fleuve décider avec son propre réglage.
 */
export async function sendToFleuve(
  url,
  { referer = null, fileName = null, title = null, quality, kind = null, focus = true, startPaused = false } = {},
) {
  const wanted = quality === undefined ? (await getSettings()).quality : quality;
  return call("/add", {
    method: "POST",
    body: JSON.stringify({
      url,
      referer,
      fileName,
      title,
      quality: wanted && wanted !== "default" ? String(wanted) : null,
      kind,
      focus,
      startPaused,
      cookie: await cookieHeader(url),
      userAgent: navigator.userAgent,
    }),
  });
}

/** Qualités proposées par une vidéo (HLS ou site vidéo), de la meilleure à la plus basse : [{height, size}]. */
export async function listQualities(url, referer = null) {
  const body = await call("/qualities", {
    method: "POST",
    body: JSON.stringify({ url, referer, cookie: await cookieHeader(url), userAgent: navigator.userAgent }),
  });
  return body.qualities || [];
}

/** Hauteurs proposées quand le site ne dit pas lesquelles il a. */
export const STANDARD_HEIGHTS = [2160, 1440, 1080, 720, 480, 360];

export function fmtSize(n) {
  if (!n) return "";
  const u = ["o", "Ko", "Mo", "Go"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i ? 1 : 0).replace(".", ",")} ${u[i]}`;
}

/**
 * Nettoie le titre d'une page pour en faire un nom de fichier : retire le nom du
 * site (« Épisode 12 - Nom du site » → « Épisode 12 ») quand il figure dans le titre.
 */
export function cleanTitle(title, pageUrl) {
  const t = (title || "").replace(/\s+/g, " ").trim();
  if (!t) return null;
  const norm = (s) =>
    s.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().replace(/[^a-z0-9]/g, "");
  let brand = "";
  try {
    const labels = new URL(pageUrl).hostname.replace(/^www\./, "").split(".");
    brand = norm(labels.length > 1 ? labels[labels.length - 2] : labels[0]);
  } catch {}
  const parts = t.split(/\s+[|\-–—·»]\s+/);
  if (parts.length > 1 && brand.length >= 3) {
    const kept = parts.filter((p) => !norm(p).includes(brand));
    if (kept.length) return kept.join(" - ");
  }
  return t;
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
