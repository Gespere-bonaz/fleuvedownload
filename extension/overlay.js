// Bouton « Télécharger cette vidéo » affiché au-dessus des vidéos de la page,
// à la manière d'IDM. Le bouton vit dans un Shadow DOM pour ne pas être
// déformé par les styles du site. La flèche « ▾ » ouvre le choix de qualité
// (720p, 1080p…) ; un clic sur le bouton lui-même utilise la qualité par défaut
// des options de l'extension.
(() => {
  if (window.__fleuveOverlay) return;
  window.__fleuveOverlay = true;

  const MIN_W = 200;
  const MIN_H = 120;
  const STANDARD_HEIGHTS = [2160, 1440, 1080, 720, 480, 360];

  const host = document.createElement("div");
  host.style.cssText = "all:initial;position:fixed;top:0;left:0;z-index:2147483647;display:none;";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `
    <style>
      :host { all: initial; }
      * { box-sizing: border-box; }
      .wrap { position: relative; font: 500 13px/1.3 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; color: #fff; }
      .bar { display: inline-flex; border-radius: 9px; box-shadow: 0 2px 10px rgba(0,0,0,.4); }
      .bar button {
        all: unset;
        box-sizing: border-box;
        display: inline-flex; align-items: center; gap: 7px;
        min-height: 32px;
        padding: 0 12px 0 10px;
        background: rgba(15, 23, 42, .9);
        border: 1px solid rgba(255,255,255,.2);
        font: 600 13px/1 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        color: #fff;
        cursor: pointer;
        user-select: none;
        white-space: nowrap;
        transition: background-color .15s;
      }
      .main { border-radius: 9px 0 0 9px; }
      .more { border-radius: 0 9px 9px 0; border-left-color: rgba(255,255,255,.12) !important; padding: 0 9px !important; }
      .bar button:hover { background: #1b6fd8; }
      .bar button:focus-visible, .item:focus-visible { outline: 2px solid #8ec0ff; outline-offset: -2px; }
      .bar.ok button { background: #1f9d55; }
      .bar.ko button { background: #c93a3a; }
      svg { width: 16px; height: 16px; flex: none; }
      .more svg { width: 14px; height: 14px; transition: transform .15s; }
      .more[aria-expanded="true"] svg { transform: rotate(180deg); }

      .menu {
        position: absolute; right: 0; top: calc(100% + 6px);
        width: max-content; min-width: 230px; max-width: min(320px, calc(100vw - 16px));
        max-height: min(320px, calc(100vh - 90px)); overflow: auto;
        padding: 4px;
        background: rgba(15, 23, 42, .97);
        border: 1px solid rgba(255,255,255,.2); border-radius: 10px;
        box-shadow: 0 8px 24px rgba(0,0,0,.5);
        animation: pop .12s ease-out;
      }
      .menu[hidden] { display: none; }
      @keyframes pop { from { opacity: 0; transform: translateY(-4px); } }
      .head { padding: 6px 10px 4px; font-size: 11px; font-weight: 700; letter-spacing: .05em; text-transform: uppercase; color: #aab3c2; }
      .sep { height: 1px; margin: 4px 6px; background: rgba(255,255,255,.14); }
      .note { padding: 6px 10px; font-size: 12px; color: #aab3c2; white-space: normal; }
      .item {
        all: unset; box-sizing: border-box;
        display: flex; align-items: center; justify-content: space-between; gap: 18px;
        width: 100%; min-height: 32px; padding: 6px 10px;
        border-radius: 7px; cursor: pointer; white-space: nowrap;
        font: 500 13px/1.2 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; color: #fff;
      }
      .item:hover, .item:focus-visible { background: #1b6fd8; }
      .item .size { color: #aab3c2; font-size: 12px; font-variant-numeric: tabular-nums; }
      .item:hover .size, .item:focus-visible .size { color: #e4eefc; }

      @media (prefers-reduced-motion: reduce) {
        .menu { animation: none; }
        .bar button, .more svg { transition: none; }
      }
    </style>
    <div class="wrap">
      <div class="bar">
        <button type="button" class="main" title="Envoyer cette vidéo à Fleuve">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>
          </svg>
          <span>Télécharger cette vidéo</span>
        </button>
        <button type="button" class="more" title="Choisir la qualité" aria-label="Choisir la qualité" aria-haspopup="menu" aria-expanded="false">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>
        </button>
      </div>
      <div class="menu" role="menu" aria-label="Qualité de la vidéo" hidden></div>
    </div>`;
  const bar = root.querySelector(".bar");
  const btn = root.querySelector(".main");
  const more = root.querySelector(".more");
  const menu = root.querySelector(".menu");
  const label = btn.querySelector("span");

  let current = null; // vidéo survolée
  let hideTimer = null;
  let busy = false;
  let menuOpen = false;

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
    if (current !== video && !busy) {
      reset();
      closeMenu();
    }
    current = video;
    mount();
    place(video);
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (busy || menuOpen) return;
      host.style.display = "none";
      current = null;
    }, 1500);
  }

  function reset() {
    bar.className = "bar";
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

  // --- Choix de la qualité ---------------------------------------------------
  function fmtSize(n) {
    if (!n) return "";
    const u = ["o", "Ko", "Mo", "Go"];
    let i = 0;
    while (n >= 1024 && i < u.length - 1) {
      n /= 1024;
      i++;
    }
    return `${n.toFixed(i ? 1 : 0).replace(".", ",")} ${u[i]}`;
  }

  function closeMenu(refocus = false) {
    const wasOpen = menuOpen;
    menuOpen = false;
    menu.hidden = true;
    menu.replaceChildren();
    more.setAttribute("aria-expanded", "false");
    if (wasOpen && refocus) more.focus();
  }

  const items = () => [...menu.querySelectorAll(".item")];

  function heading(text) {
    const el = document.createElement("div");
    el.className = "head";
    el.textContent = text;
    return el;
  }

  function note(text) {
    const el = document.createElement("div");
    el.className = "note";
    el.textContent = text;
    return el;
  }

  function item(text, size, quality) {
    const el = document.createElement("button");
    el.type = "button";
    el.className = "item";
    el.setAttribute("role", "menuitem");
    const l = document.createElement("span");
    l.textContent = text;
    el.append(l);
    if (size) {
      const s = document.createElement("span");
      s.className = "size";
      s.textContent = size;
      el.append(s);
    }
    el.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
      send(quality);
    });
    return el;
  }

  async function openMenu() {
    if (!current || busy) return;
    menuOpen = true;
    menu.hidden = false;
    more.setAttribute("aria-expanded", "true");
    menu.replaceChildren(heading("Qualité"), note("Recherche des qualités disponibles…"));
    const video = current;
    let res;
    try {
      res = await chrome.runtime.sendMessage({
        type: "fleuve-qualities",
        src: video.currentSrc || video.src || "",
        pageUrl: location.href,
      });
    } catch (e) {
      res = { ok: false, error: e.message };
    }
    if (!menuOpen) return; // refermé pendant la recherche
    menu.replaceChildren(heading("Qualité"));
    const list = res?.ok ? res.qualities : [];
    if (res?.ok && res.kind === "file") {
      menu.append(note("Fichier vidéo direct : une seule qualité."), item("Télécharger", "", undefined));
    } else if (list.length) {
      menu.append(item("Meilleure disponible", `${list[0].height}p`, "best"), document.createElement("div"));
      menu.lastChild.className = "sep";
      for (const q of list) menu.append(item(`${q.height}p`, fmtSize(q.size), String(q.height)));
    } else {
      menu.append(
        note(
          res?.ok
            ? "Une seule qualité annoncée par le site."
            : /fermé|fetch|répond|Jeton/i.test(res?.error || "")
              ? "Fleuve est fermé ou l'extension n'est pas connectée."
              : "Qualités non détectées : hauteurs courantes proposées.",
        ),
        item("Meilleure disponible", "", "best"),
      );
      if (!res?.ok || res.kind !== "hls") for (const h of STANDARD_HEIGHTS) menu.append(item(`${h}p au plus`, "", String(h)));
    }
    items()[0]?.focus({ preventScroll: true });
  }

  more.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (menuOpen) closeMenu();
    else openMenu();
  });
  document.addEventListener(
    "click",
    (e) => {
      if (menuOpen && !e.composedPath().includes(host)) closeMenu();
    },
    { capture: true },
  );

  // Clavier : flèches pour parcourir, Échap pour fermer. Les touches ne doivent
  // pas atteindre le lecteur de la page (les flèches y règlent le son et la position).
  root.addEventListener("keydown", (e) => {
    if (!menuOpen) return;
    const list = items();
    const i = list.indexOf(root.activeElement);
    let handled = true;
    if (e.key === "ArrowDown") list[(i + 1) % list.length]?.focus();
    else if (e.key === "ArrowUp") list[(i - 1 + list.length) % list.length]?.focus();
    else if (e.key === "Home") list[0]?.focus();
    else if (e.key === "End") list[list.length - 1]?.focus();
    else if (e.key === "Escape") closeMenu(true);
    else handled = false;
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  });

  // --- Envoi -----------------------------------------------------------------
  async function send(quality) {
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
        quality, // undefined : qualité par défaut des options
      });
      if (!res || !res.ok) throw new Error(res?.error || "Fleuve ne répond pas");
      bar.className = "bar ok";
      label.textContent = "Envoyé à Fleuve ✓";
    } catch (err) {
      bar.className = "bar ko";
      label.textContent = /fermé|fetch|répond/i.test(err.message) ? "Fleuve est fermé" : err.message.slice(0, 60);
    }
    setTimeout(() => {
      busy = false;
      reset();
      scheduleHide();
    }, 2500);
  }

  btn.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
    send(undefined);
  });
})();
