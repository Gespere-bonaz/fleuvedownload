// Bouton « Télécharger cette vidéo » affiché au-dessus des vidéos de la page,
// à la manière d'IDM. Le bouton vit dans un Shadow DOM pour ne pas être
// déformé par les styles du site.
(() => {
  if (window.__fleuveOverlay) return;
  window.__fleuveOverlay = true;

  const MIN_W = 200;
  const MIN_H = 120;

  const host = document.createElement("div");
  host.style.cssText = "all:initial;position:fixed;top:0;left:0;z-index:2147483647;display:none;";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `
    <style>
      button {
        all: initial;
        display: inline-flex; align-items: center; gap: 7px;
        padding: 7px 12px 7px 9px;
        border-radius: 8px;
        background: rgba(15, 23, 42, 0.88);
        color: #fff;
        font: 600 13px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        box-shadow: 0 2px 10px rgba(0,0,0,.35);
        border: 1px solid rgba(255,255,255,.18);
        cursor: pointer;
        user-select: none;
        transition: background .15s;
        white-space: nowrap;
      }
      button:hover { background: #1b6fd8; }
      button.ok { background: #1f9d55; }
      button.ko { background: #d64545; }
      svg { width: 16px; height: 16px; flex: none; }
    </style>
    <button type="button" title="Envoyer cette vidéo à Fleuve">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>
      </svg>
      <span>Télécharger cette vidéo</span>
    </button>`;
  const btn = root.querySelector("button");
  const label = root.querySelector("span");

  let current = null; // vidéo survolée
  let hideTimer = null;
  let busy = false;

  function mount() {
    const parent = document.fullscreenElement || document.documentElement;
    if (host.parentNode !== parent) parent.appendChild(host);
  }

  function place(video) {
    const r = video.getBoundingClientRect();
    host.style.display = "block";
    const w = host.offsetWidth || 190;
    host.style.left = `${Math.max(4, r.right - w - 12)}px`;
    host.style.top = `${Math.max(4, r.top + 12)}px`;
  }

  function show(video) {
    clearTimeout(hideTimer);
    if (current !== video && !busy) reset();
    current = video;
    mount();
    place(video);
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (busy) return;
      host.style.display = "none";
      current = null;
    }, 1500);
  }

  function reset() {
    btn.className = "";
    label.textContent = "Télécharger cette vidéo";
  }

  function videoAt(x, y) {
    for (const v of document.querySelectorAll("video")) {
      const r = v.getBoundingClientRect();
      if (r.width < MIN_W || r.height < MIN_H) continue;
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return v;
    }
    return null;
  }

  let pending = false;
  document.addEventListener(
    "mousemove",
    (e) => {
      if (pending) return;
      pending = true;
      requestAnimationFrame(() => {
        pending = false;
        if (e.composedPath().includes(host)) {
          clearTimeout(hideTimer);
          return;
        }
        const v = videoAt(e.clientX, e.clientY);
        if (v) show(v);
        else if (current) scheduleHide();
      });
    },
    { passive: true, capture: true },
  );
  window.addEventListener("scroll", () => current && place(current), { passive: true, capture: true });
  document.addEventListener("fullscreenchange", () => current && (mount(), place(current)));

  btn.addEventListener("click", async (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (!current || busy) return;
    busy = true;
    label.textContent = "Envoi…";
    const src = current.currentSrc || current.src || "";
    try {
      const res = await chrome.runtime.sendMessage({
        type: "fleuve-video",
        src,
        pageUrl: location.href,
        title: document.title,
      });
      if (!res || !res.ok) throw new Error(res?.error || "Fleuve ne répond pas");
      btn.className = "ok";
      label.textContent = "Envoyé à Fleuve ✓";
    } catch (err) {
      btn.className = "ko";
      label.textContent = /fermé|fetch|répond/i.test(err.message) ? "Fleuve est fermé" : err.message.slice(0, 60);
    }
    setTimeout(() => {
      busy = false;
      reset();
      scheduleHide();
    }, 2500);
  });
})();
