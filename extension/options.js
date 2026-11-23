import { getSettings, pairStart, pairStatus, ping } from "./common.js";

const $ = (id) => document.getElementById(id);
const fields = ["token", "port", "minSizeMb", "extensions", "quality"];

/** Message de l'encadré « Connexion » ; `tone` : "good", "bad" ou rien. */
function connMsg(text, tone = "") {
  const el = $("conn-msg");
  el.textContent = text;
  el.className = `muted ${tone}`.trim();
}

async function load() {
  const s = await getSettings();
  for (const f of fields) $(f).value = s[f];
  $("intercept").checked = s.intercept;
}

async function save() {
  await chrome.storage.local.set({
    token: $("token").value.trim(),
    port: Number($("port").value) || 47821,
    minSizeMb: Math.max(0, Number($("minSizeMb").value) || 0),
    extensions: $("extensions").value.trim().toLowerCase(),
    quality: $("quality").value,
    intercept: $("intercept").checked,
  });
}

let savedTimer;
$("save").onclick = async () => {
  await save();
  $("msg").textContent = "Enregistré ✓";
  clearTimeout(savedTimer);
  savedTimer = setTimeout(() => ($("msg").textContent = ""), 2500);
};

$("test").onclick = async () => {
  await save();
  connMsg("Test en cours…");
  try {
    const r = await ping();
    connMsg(`Connecté à Fleuve ${r.version}.`, "good");
  } catch (e) {
    connMsg(
      /fetch|network/i.test(e.message) ? "Fleuve ne répond pas : ouvrez l'application, puis réessayez." : `Échec : ${e.message}`,
      "bad",
    );
  }
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Appairage : Fleuve demande l'accord de l'utilisateur, puis renvoie le jeton.
let cancelPairing = null;
$("pair-cancel").onclick = () => cancelPairing?.();

$("pair").onclick = async () => {
  const port = Number($("port").value) || 47821;
  $("pair").disabled = true;
  connMsg("");
  let cancelled = false;
  cancelPairing = () => (cancelled = true);
  try {
    const { id, code, expiresIn } = await pairStart(port);
    $("pair-code").textContent = code;
    $("pair-box").hidden = false;
    // Sur une fenêtre peu haute, l'encadré pourrait rester hors de vue.
    $("pair-box").scrollIntoView({ block: "center" });
    const deadline = Date.now() + (expiresIn + 5) * 1000;
    let state = "expired";
    while (Date.now() < deadline && !cancelled) {
      $("pair-left").textContent = `Expire dans ${Math.max(0, Math.round((deadline - Date.now()) / 1000) - 5)} s`;
      await sleep(1000);
      const r = await pairStatus(port, id);
      state = r.state;
      if (state === "approved") {
        $("token").value = r.token;
        await save();
        const p = await ping();
        connMsg(`Connecté à Fleuve ${p.version}.`, "good");
        return;
      }
      if (state !== "pending") break;
    }
    if (cancelled) connMsg("Connexion annulée. Pensez à refuser la demande dans Fleuve.");
    else if (state === "denied") connMsg("Demande refusée dans Fleuve.", "bad");
    else connMsg("La demande a expiré : cliquez de nouveau sur « Connecter à Fleuve ».", "bad");
  } catch (e) {
    connMsg(
      /fetch|network/i.test(e.message) ? "Fleuve ne répond pas : ouvrez l'application, puis réessayez." : `Échec : ${e.message}`,
      "bad",
    );
  } finally {
    cancelPairing = null;
    $("pair-box").hidden = true;
    $("pair").disabled = false;
  }
};

load();
