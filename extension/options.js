import { getSettings, ping } from "./common.js";

const $ = (id) => document.getElementById(id);
const fields = ["token", "port", "minSizeMb", "extensions"];

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
    intercept: $("intercept").checked,
  });
}

$("save").onclick = async () => {
  await save();
  $("msg").textContent = "Enregistré.";
};
$("test").onclick = async () => {
  await save();
  try {
    const r = await ping();
    $("msg").textContent = `Connecté à Fleuve ${r.version}.`;
  } catch (e) {
    $("msg").textContent = `Échec : ${e.message}`;
  }
};
load();
