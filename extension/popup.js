import {
  STANDARD_HEIGHTS,
  cleanTitle,
  fmtSize,
  getSettings,
  isVideoSite,
  listQualities,
  ping,
  sendToFleuve,
} from "./common.js";

const $ = (id) => document.getElementById(id);
const msg = (t, bad = false) => {
  $("msg").textContent = t;
  $("msg").style.color = bad ? "var(--bad)" : "";
};

const MIME_LABELS = {
  "video/mp4": "MP4",
  "video/webm": "WebM",
  "video/x-matroska": "MKV",
  "video/quicktime": "MOV",
  "audio/mpeg": "MP3",
  "audio/mp4": "M4A",
  "audio/aac": "AAC",
  "audio/ogg": "OGG",
  "audio/flac": "FLAC",
  "audio/wav": "WAV",
};
/** « video/mp4 » → « MP4 » ; type inconnu → son sous-type en majuscules. */
function typeLabel(mime) {
  const m = (mime || "").toLowerCase();
  return MIME_LABELS[m] || (m.split("/")[1] || "").replace(/^x-/, "").toUpperCase();
}

/**
 * Liste déroulante de qualité d'une vidéo. Les qualités réellement proposées sont
 * cherchées quand on l'ouvre (une seule fois) ; sinon, hauteurs courantes.
 * `value()` donne le choix : undefined = qualité par défaut des options.
 */
function qualityPicker(getTarget) {
  const sel = document.createElement("select");
  sel.className = "quality";
  sel.title = "Qualité de la vidéo";
  sel.setAttribute("aria-label", "Qualité de la vidéo");
  const add = (value, label) => sel.append(new Option(label, value));
  add("", "Qualité par défaut");
  let loaded = false;
  const load = async () => {
    if (loaded) return;
    loaded = true;
    const keep = sel.value;
    sel.options[0].textContent = "Recherche des qualités…";
    let list = [];
    let failed = false;
    try {
      const { url, referer } = getTarget();
      list = await listQualities(url, referer);
    } catch {
      failed = true;
    }
    sel.replaceChildren();
    add("", "Qualité par défaut");
    add("best", list[0] ? `Meilleure (${list[0].height}p${list[0].size ? ` · ${fmtSize(list[0].size)}` : ""})` : "Meilleure");
    if (list.length) {
      for (const q of list) add(String(q.height), `${q.height}p${q.size ? ` · ${fmtSize(q.size)}` : ""}`);
    } else {
      for (const h of STANDARD_HEIGHTS) add(String(h), `${h}p au plus`);
      sel.title = failed ? "Qualités non détectées : hauteurs courantes proposées" : "Une seule qualité annoncée";
    }
    sel.value = [...sel.options].some((o) => o.value === keep) ? keep : "";
  };
  // Dès que la souris approche, pour que la liste soit prête au moment du clic.
  for (const ev of ["mouseenter", "focus", "mousedown"]) sel.addEventListener(ev, load);
  return { el: sel, value: () => sel.value || undefined };
}

async function init() {
  const s = await getSettings();
  $("intercept").checked = s.intercept;
  $("intercept").onchange = (e) => chrome.storage.local.set({ intercept: e.target.checked });
  $("options").onclick = () => chrome.runtime.openOptionsPage();

  const st = $("status");
  try {
    const r = await ping();
    st.className = "status ok";
    st.lastElementChild.textContent = `Connecté · v${r.version}`;
  } catch (e) {
    st.className = "status ko";
    st.lastElementChild.textContent = s.token ? "Fleuve est fermé" : "Non connecté : cliquer ici";
    if (!s.token) {
      // → page d'options, bouton « Connecter à Fleuve ». Utilisable au clavier.
      const open = () => chrome.runtime.openOptionsPage();
      st.setAttribute("role", "button");
      st.tabIndex = 0;
      st.onclick = open;
      st.onkeydown = (ev) => (ev.key === "Enter" || ev.key === " ") && (ev.preventDefault(), open());
    }
  }

  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });

  // Lien collé : la qualité n'est proposée que pour une vidéo (HLS ou site vidéo).
  const isVideoLink = (u) => /\.m3u8(\?|$)/i.test(u) || isVideoSite(u);
  const linkQuality = qualityPicker(() => ({ url: $("link").value.trim(), referer: tab?.url || null }));
  linkQuality.el.hidden = true;
  $("quality-slot").append(linkQuality.el);
  $("link").addEventListener("input", () => {
    linkQuality.el.hidden = !isVideoLink($("link").value.trim());
  });

  $("send-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const url = $("link").value.trim();
    if (!/^https?:\/\//i.test(url)) return msg("Lien http(s) attendu.", true);
    try {
      await sendToFleuve(url, {
        referer: tab?.url || null,
        kind: /\.m3u8(\?|$)/i.test(url) ? "hls" : isVideoSite(url) ? "site" : null,
        quality: linkQuality.el.hidden ? undefined : linkQuality.value(),
      });
      msg("Envoyé à Fleuve.");
      $("link").value = "";
      linkQuality.el.hidden = true;
    } catch (e) {
      msg(e.message, true);
    }
  });

  if (!tab) return;
  const key = `tab:${tab.id}`;
  const stored = (await chrome.storage.session.get(key))[key] || [];
  // Sur YouTube & co, la page elle-même est la vidéo (les flux ne sont pas listés).
  const list = isVideoSite(tab.url || "") ? [{ url: tab.url, kind: "site", type: "Page vidéo" }, ...stored] : stored;
  $("empty").hidden = list.length > 0;
  $("count").textContent = list.length ? String(list.length) : "";
  const title = cleanTitle(tab.title, tab.url);
  const ul = $("media");
  for (const m of list) {
    const li = document.createElement("li");
    const info = document.createElement("div");
    info.className = "info";

    // Ce qui est affiché est ce qui nommera le fichier : le titre de la page pour
    // un flux ou une page vidéo, le nom du fichier sinon.
    const fileName = decodeURIComponent(new URL(m.url).pathname.split("/").pop() || m.url);
    const display = m.kind === "file" ? fileName : title || fileName;
    const name = document.createElement("div");
    name.className = "name";
    const tag = document.createElement("span");
    tag.className = `tag ${m.kind}`;
    tag.textContent = m.kind === "hls" ? "HLS" : m.kind === "site" ? "SITE" : "FICHIER";
    const text = document.createElement("span");
    text.className = "text";
    text.textContent = display;
    text.title = m.url;
    name.append(tag, text);

    const meta = document.createElement("div");
    meta.className = "muted meta";
    meta.textContent =
      m.kind === "hls"
        ? "Flux vidéo"
        : m.kind === "site"
          ? "Page vidéo"
          : [typeLabel(m.type), fmtSize(m.size)].filter(Boolean).join(" · ");
    info.append(name, meta);

    // Qualité : seulement pour les flux HLS et les pages de sites vidéo.
    const picker = m.kind === "file" ? null : qualityPicker(() => ({ url: m.url, referer: tab.url }));
    if (picker) info.append(picker.el);

    const btn = document.createElement("button");
    btn.textContent = "Télécharger";
    btn.onclick = async () => {
      try {
        // Le titre de la page nomme le fichier (vidéos et sons ; HLS toujours).
        const isMedia = m.kind === "hls" || /^(video|audio)\//i.test(m.type || "");
        await sendToFleuve(m.url, {
          referer: tab.url,
          kind: m.kind === "hls" ? "hls" : m.kind === "site" ? "site" : null,
          title: isMedia && m.kind !== "site" ? title : null,
          quality: picker ? picker.value() : undefined,
        });
        btn.textContent = "Envoyé ✓";
        btn.disabled = true;
      } catch (e) {
        msg(e.message, true);
      }
    };
    li.append(info, btn);
    ul.append(li);
  }
}

init();
