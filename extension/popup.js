import { getSettings, isVideoSite, ping, sendToFleuve } from "./common.js";

const $ = (id) => document.getElementById(id);
const msg = (t, bad = false) => {
  $("msg").textContent = t;
  $("msg").style.color = bad ? "#d64545" : "";
};

function fmtSize(n) {
  if (!n) return "";
  const u = ["o", "Ko", "Mo", "Go"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(i ? 1 : 0).replace(".", ",")} ${u[i]}`;
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
    st.lastElementChild.textContent = s.token ? "Fleuve est fermé" : "Jeton manquant";
  }

  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });

  $("send").onclick = async () => {
    const url = $("link").value.trim();
    if (!/^https?:\/\//i.test(url)) return msg("Lien http(s) attendu.", true);
    try {
      await sendToFleuve(url, {
        referer: tab?.url || null,
        kind: /\.m3u8(\?|$)/i.test(url) ? "hls" : isVideoSite(url) ? "site" : null,
      });
      msg("Envoyé à Fleuve.");
      $("link").value = "";
    } catch (e) {
      msg(e.message, true);
    }
  };

  if (!tab) return;
  const key = `tab:${tab.id}`;
  const list = (await chrome.storage.session.get(key))[key] || [];
  $("empty").hidden = list.length > 0;
  const ul = $("media");
  for (const m of list) {
    const li = document.createElement("li");
    const info = document.createElement("div");
    info.className = "info";
    const name = decodeURIComponent(new URL(m.url).pathname.split("/").pop() || m.url);
    info.innerHTML = `<div class="url"></div><div class="muted"></div>`;
    info.firstChild.innerHTML = `<span class="tag ${m.kind}">${m.kind === "hls" ? "HLS" : "FICHIER"}</span>`;
    info.firstChild.append(name);
    info.firstChild.title = m.url;
    info.lastChild.textContent = [m.type, fmtSize(m.size)].filter(Boolean).join(" · ");
    const btn = document.createElement("button");
    btn.textContent = "Télécharger";
    btn.onclick = async () => {
      try {
        await sendToFleuve(m.url, { referer: tab.url, kind: m.kind === "hls" ? "hls" : null });
        btn.textContent = "Envoyé";
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
