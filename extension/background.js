import { extOf, getSettings, isStreamFragment, isVideoSite, sendToFleuve } from "./common.js";

// --- Menu contextuel « Télécharger avec Fleuve » ---------------------------
chrome.runtime.onInstalled.addListener(async () => {
  await chrome.contextMenus.removeAll();
  chrome.contextMenus.create({
    id: "fleuve-download",
    title: "Télécharger avec Fleuve",
    contexts: ["link", "video", "audio", "image"],
  });
  chrome.contextMenus.create({
    id: "browser-download",
    title: "Télécharger avec le navigateur (sans Fleuve)",
    contexts: ["link"],
  });
  const { token } = await getSettings();
  if (!token) chrome.runtime.openOptionsPage();
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "browser-download") {
    chrome.downloads.download({ url: info.linkUrl });
    return;
  }
  try {
    if (info.mediaType === "video" || info.mediaType === "audio") {
      await sendVideo({ src: info.srcUrl || "", pageUrl: info.frameUrl || info.pageUrl, tabId: tab?.id });
      flash("OK", "#1f9d55");
      return;
    }
    // Sur une image, on veut l'image elle-même, pas le lien qui l'entoure
    // (sur Google Images, ce lien mène souvent vers une page ou une chaîne).
    const url = info.mediaType === "image" ? info.srcUrl : info.linkUrl || info.srcUrl;
    if (!url) return;
    if (/^(data|blob):/i.test(url)) {
      // Image intégrée à la page : seul le navigateur peut l'enregistrer.
      chrome.downloads.download({ url });
      return;
    }
    if (!/^https?:/i.test(url)) return;
    await sendToFleuve(url, {
      referer: info.pageUrl,
      kind: extOf(url) === "m3u8" ? "hls" : isVideoSite(url) ? "site" : null,
    });
    flash("OK", "#1f9d55");
  } catch (e) {
    console.warn("Fleuve :", e.message);
    flash("!", "#d64545");
  }
});

function flash(text, color) {
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setBadgeText({ text });
  setTimeout(() => chrome.action.setBadgeText({ text: "" }), 2500);
}

// --- Bouton « Télécharger cette vidéo » posé sur les vidéos -----------------
/**
 * Choisit la bonne façon de récupérer une vidéo :
 * 1. page d'un site vidéo (YouTube…) → moteur yt-dlp de Fleuve ;
 * 2. vidéo avec un vrai lien de fichier → téléchargement direct multi-connexions ;
 * 3. flux HLS repéré dans l'onglet → moteur HLS ;
 * 4. sinon → yt-dlp tente de trouver la vidéo dans la page.
 */
async function sendVideo({ src, pageUrl, tabId }) {
  if (isVideoSite(pageUrl)) return sendToFleuve(pageUrl, { kind: "site" });
  if (/^https?:/i.test(src) && !isStreamFragment(src)) {
    return sendToFleuve(src, { referer: pageUrl, kind: extOf(src) === "m3u8" ? "hls" : "file" });
  }
  if (tabId != null) {
    const list = (await chrome.storage.session.get(`tab:${tabId}`))[`tab:${tabId}`] || [];
    const best = list.find((m) => m.kind === "hls") || list.find((m) => m.kind === "file");
    if (best) return sendToFleuve(best.url, { referer: pageUrl, kind: best.kind });
  }
  return sendToFleuve(pageUrl, { kind: "site" });
}

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  if (msg?.type !== "fleuve-video") return;
  sendVideo({ src: msg.src, pageUrl: msg.pageUrl || sender.url, tabId: sender.tab?.id })
    .then(() => {
      flash("↓", "#1b6fd8");
      reply({ ok: true });
    })
    .catch((e) => reply({ ok: false, error: e.message }));
  return true; // réponse asynchrone
});

// --- Interception des téléchargements du navigateur -------------------------
// Comme IDM : tout téléchargement lancé dans le navigateur qui correspond aux
// règles est repris par Fleuve et ajouté à sa liste ; le navigateur annule le
// sien. Si Fleuve est fermé ou refuse, le navigateur continue normalement.
//
// On attend `onDeterminingFilename` plutôt que `onCreated` : à ce moment le
// navigateur connaît le vrai nom du fichier, son type et souvent sa taille,
// même quand le lien ne contient aucune extension (/download?id=…).

const INTERESTING_MIME =
  /^(video|audio)\/|^application\/(octet-stream|zip|x-|gzip|pdf|epub|java-archive|vnd\.(android|apple|microsoft|ms-|openxmlformats|rar))/i;
const handled = new Set();

function wanted(item, s) {
  const name = (item.filename || "").split(/[\\/]/).pop();
  const url = item.finalUrl || item.url;
  const ext = extOf("http://x/" + encodeURIComponent(name)) || extOf(url);
  const exts = s.extensions.split(/[\s,]+/).filter(Boolean);
  const size = Math.max(item.totalBytes || 0, item.fileSize || 0);
  const mime = item.mime || "";
  if (/^text\/html/i.test(mime) || /^image\//i.test(mime)) return exts.includes(ext);
  return exts.includes(ext) || size > s.minSizeMb * 1024 * 1024 || INTERESTING_MIME.test(mime);
}

async function takeOver(item) {
  if (handled.has(item.id)) return;
  handled.add(item.id);
  setTimeout(() => handled.delete(item.id), 60_000);
  const s = await getSettings();
  if (!s.intercept || !s.token) return;
  if (item.byExtensionId === chrome.runtime.id) return; // « sans Fleuve »
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url)) return; // blob:, data: → le navigateur garde la main
  if (!wanted(item, s)) return;

  // Pause pendant qu'on demande à Fleuve ; s'il ne répond pas, le navigateur reprend.
  try {
    await chrome.downloads.pause(item.id);
  } catch {}
  const fileName = (item.filename || "").split(/[\\/]/).pop() || null;
  try {
    // Lancé directement dans Fleuve, sans fenêtre ni mise au premier plan.
    await sendToFleuve(url, {
      referer: item.referrer || null,
      fileName,
      kind: extOf(url) === "m3u8" ? "hls" : "file",
      focus: false,
    });
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
    flash("↓", "#1b6fd8");
  } catch (e) {
    console.warn("Fleuve indisponible, téléchargement laissé au navigateur :", e.message);
    try {
      await chrome.downloads.resume(item.id);
    } catch {}
  }
}

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  suggest(); // on garde le nom proposé par le navigateur
  takeOver(item);
});

// Filet de sécurité : certains téléchargements (ex. lancés par d'autres
// extensions) ne passent pas par onDeterminingFilename.
chrome.downloads.onCreated.addListener((item) => {
  setTimeout(async () => {
    const [cur] = await chrome.downloads.search({ id: item.id });
    if (cur && cur.state === "in_progress" && cur.filename) takeOver(cur);
  }, 1500);
});

// --- Détection des vidéos et sons dans les onglets --------------------------
const MEDIA_TYPES = /^(video|audio)\//i;
const HLS_TYPES = /mpegurl/i;
const SKIP_EXT = new Set(["ts", "m4s", "aac", "vtt", "key"]); // morceaux, sous-titres, clés

function header(details, name) {
  const h = (details.responseHeaders || []).find((x) => x.name.toLowerCase() === name);
  return h ? h.value : "";
}

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (details.tabId < 0 || details.statusCode >= 400) return;
    if (isStreamFragment(details.url)) return; // morceaux de flux YouTube & co
    const type = header(details, "content-type");
    const ext = extOf(details.url);
    let kind = null;
    if (ext === "m3u8" || HLS_TYPES.test(type)) kind = "hls";
    else if (MEDIA_TYPES.test(type) && !SKIP_EXT.has(ext)) {
      const size = Number(header(details, "content-length")) || 0;
      const range = header(details, "content-range");
      const total = range ? Number(range.split("/")[1]) || size : size;
      if (total > 512 * 1024) kind = "file";
      else return;
    }
    if (!kind) return;
    remember(details.tabId, {
      url: details.url,
      kind,
      type: type.split(";")[0],
      size: Number((header(details, "content-range").split("/")[1] || header(details, "content-length"))) || null,
    });
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other"] },
  ["responseHeaders"],
);

async function remember(tabId, media) {
  const key = `tab:${tabId}`;
  const store = await chrome.storage.session.get(key);
  const list = store[key] || [];
  if (list.some((m) => m.url === media.url)) return;
  list.unshift({ ...media, at: Date.now() });
  list.length = Math.min(list.length, 30);
  await chrome.storage.session.set({ [key]: list });
  chrome.action.setBadgeBackgroundColor({ tabId, color: "#1b6fd8" });
  chrome.action.setBadgeText({ tabId, text: String(list.length) });
}

chrome.tabs.onRemoved.addListener((tabId) => chrome.storage.session.remove(`tab:${tabId}`));
chrome.tabs.onUpdated.addListener((tabId, change) => {
  // Nouvelle page : on oublie les médias de l'ancienne.
  if (change.status === "loading" && change.url) {
    chrome.storage.session.remove(`tab:${tabId}`);
    chrome.action.setBadgeText({ tabId, text: "" });
  }
});
