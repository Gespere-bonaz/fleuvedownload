import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import { QualitySelect, initialQuality } from "./Quality";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { AddRequest, PairingInfo, Settings } from "../types";

function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="modal__backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={title}>
        <h2 className="modal__title">{title}</h2>
        {children}
      </div>
    </div>
  );
}

/** Valeur d'un <input type="datetime-local"> → horodatage Unix (s). */
function localToUnix(v: string): number | null {
  if (!v) return null;
  const t = new Date(v).getTime();
  return Number.isNaN(t) ? null : Math.floor(t / 1000);
}

function inOneHour(): string {
  const d = new Date(Date.now() + 3600_000);
  d.setSeconds(0, 0);
  const off = d.getTimezoneOffset();
  return new Date(d.getTime() - off * 60_000).toISOString().slice(0, 16);
}

async function pickFolder(current?: string): Promise<string | null> {
  const dir = await openDialog({ directory: true, multiple: false, defaultPath: current });
  return typeof dir === "string" ? dir : null;
}

export function AddDialog({
  initialUrl,
  defaultDir,
  defaultQuality,
  onClose,
  onAdd,
}: {
  initialUrl: string;
  defaultDir: string;
  /** Réglage « Qualité vidéo » (« ask », « best », « 720 »…). */
  defaultQuality: string;
  onClose: () => void;
  onAdd: (reqs: AddRequest[]) => Promise<void>;
}) {
  const [urls, setUrls] = useState(initialUrl);
  const [fileName, setFileName] = useState("");
  const [dir, setDir] = useState<string | null>(null);
  const [when, setWhen] = useState<"now" | "later" | "paused">("now");
  const [at, setAt] = useState(inOneHour());
  const [referer, setReferer] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  useEffect(() => ref.current?.focus(), []);

  const lines = urls.split(/\s+/).map((l) => l.trim()).filter(Boolean);

  // Vidéo (flux HLS ou site vidéo) : on propose la qualité.
  const [quality, setQuality] = useState(initialQuality(defaultQuality));
  const [videoUrls, setVideoUrls] = useState<string[]>([]);
  const linesKey = lines.join("\n");
  useEffect(() => {
    const t = window.setTimeout(() => {
      Promise.all(lines.map((u) => api.isVideoUrl(u).then((v) => (v ? u : null)).catch(() => null))).then((r) =>
        setVideoUrls(r.filter((u): u is string => u !== null)),
      );
    }, 300);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linesKey]);
  const singleVideo = lines.length === 1 && videoUrls.length === 1 ? videoUrls[0] : null;
  const ref0 = referer.trim();
  const [probeKey, setProbeKey] = useState({ url: "", referer: "" });
  useEffect(() => {
    // Détection lancée après une courte pause de saisie.
    const t = window.setTimeout(() => setProbeKey({ url: singleVideo ?? "", referer: ref0 }), 500);
    return () => window.clearTimeout(t);
  }, [singleVideo, ref0]);
  const load = useMemo(
    () => (probeKey.url ? () => api.probeQualities(probeKey.url, probeKey.referer) : null),
    [probeKey],
  );

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (lines.length === 0) return setError("Collez au moins un lien.");
    const scheduledAt = when === "later" ? localToUnix(at) : null;
    if (when === "later" && scheduledAt == null) return setError("Date de démarrage invalide.");
    setBusy(true);
    try {
      await onAdd(
        lines.map((url) => ({
          url,
          fileName: lines.length === 1 && fileName.trim() ? fileName.trim() : null,
          destDir: dir,
          headers: referer.trim() ? { referer: referer.trim() } : {},
          scheduledAt,
          startPaused: when === "paused",
          quality: videoUrls.length > 0 ? quality : null,
        })),
      );
      onClose();
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <Modal title="Ajouter des téléchargements" onClose={onClose}>
      <form onSubmit={submit} className="form">
        <label className="field">
          <span>Liens (un par ligne)</span>
          <textarea
            ref={ref}
            rows={4}
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            placeholder="https://exemple.com/fichier.zip"
            spellCheck={false}
          />
        </label>
        {lines.length <= 1 && (
          <label className="field">
            <span>Nom du fichier (facultatif)</span>
            <input value={fileName} onChange={(e) => setFileName(e.target.value)} placeholder="Déduit automatiquement" />
          </label>
        )}
        {videoUrls.length > 0 && (
          <label className="field">
            <span>Qualité vidéo</span>
            <QualitySelect load={singleVideo ? load : null} value={quality} onChange={setQuality} />
          </label>
        )}
        <div className="field">
          <span>Dossier</span>
          <div className="row">
            <code className="path">{dir ?? `${defaultDir} (selon la catégorie)`}</code>
            <button type="button" className="btn" onClick={async () => setDir((await pickFolder(dir ?? defaultDir)) ?? dir)}>
              Choisir…
            </button>
          </div>
        </div>
        <fieldset className="field">
          <span>Démarrage</span>
          <div className="segmented">
            {(
              [
                ["now", "Maintenant"],
                ["later", "Programmer"],
                ["paused", "En pause"],
              ] as const
            ).map(([k, label]) => (
              <button type="button" key={k} className={when === k ? "on" : ""} onClick={() => setWhen(k)}>
                {label}
              </button>
            ))}
          </div>
          {when === "later" && <input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} />}
        </fieldset>
        <details>
          <summary>Options avancées</summary>
          <label className="field">
            <span>Page d'origine (Referer)</span>
            <input value={referer} onChange={(e) => setReferer(e.target.value)} placeholder="https://…" />
          </label>
        </details>
        {error && <p className="error">{error}</p>}
        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Annuler
          </button>
          <button type="submit" className="btn btn--primary" disabled={busy || lines.length === 0}>
            {lines.length > 1 ? `Ajouter ${lines.length} liens` : "Télécharger"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

export function ScheduleDialog({ onClose, onSchedule }: { onClose: () => void; onSchedule: (at: number) => Promise<void> }) {
  const [at, setAt] = useState(inOneHour());
  const [error, setError] = useState<string | null>(null);
  return (
    <Modal title="Programmer le téléchargement" onClose={onClose}>
      <form
        className="form"
        onSubmit={async (e) => {
          e.preventDefault();
          const t = localToUnix(at);
          if (t == null) return setError("Date invalide.");
          try {
            await onSchedule(t);
            onClose();
          } catch (err) {
            setError(String(err));
          }
        }}
      >
        <label className="field">
          <span>Démarrer le</span>
          <input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} autoFocus />
        </label>
        <p className="hint">Fleuve doit rester ouvert pour lancer le téléchargement à l'heure dite.</p>
        {error && <p className="error">{error}</p>}
        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Annuler
          </button>
          <button type="submit" className="btn btn--primary">
            Programmer
          </button>
        </div>
      </form>
    </Modal>
  );
}

export function SettingsDialog({
  initial,
  onClose,
  onSave,
}: {
  initial: Settings;
  onClose: () => void;
  onSave: (s: Settings) => Promise<void>;
}) {
  const [s, setS] = useState<Settings>(initial);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const num = (k: keyof Settings) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setS({ ...s, [k]: Math.max(0, Number(e.target.value) || 0) });

  return (
    <Modal title="Réglages" onClose={onClose}>
      <form
        className="form"
        onSubmit={async (e) => {
          e.preventDefault();
          try {
            await onSave(s);
            onClose();
          } catch (err) {
            setError(String(err));
          }
        }}
      >
        <div className="field">
          <span>Dossier de téléchargement</span>
          <div className="row">
            <code className="path">{s.downloadDir}</code>
            <button
              type="button"
              className="btn"
              onClick={async () => {
                const d = await pickFolder(s.downloadDir);
                if (d) setS({ ...s, downloadDir: d });
              }}
            >
              Choisir…
            </button>
          </div>
        </div>
        <label className="check">
          <input type="checkbox" checked={s.sortByCategory} onChange={(e) => setS({ ...s, sortByCategory: e.target.checked })} />
          Ranger dans des sous-dossiers par catégorie (Vidéos, Musique, Documents…)
        </label>
        <label className="field">
          <span>Qualité des vidéos</span>
          <select value={s.videoQuality} onChange={(e) => setS({ ...s, videoQuality: e.target.value })}>
            <option value="ask">Demander à chaque fois</option>
            <option value="best">Toujours la meilleure</option>
            {[2160, 1440, 1080, 720, 480, 360].map((h) => (
              <option key={h} value={String(h)}>
                {h}p au plus
              </option>
            ))}
          </select>
        </label>
        <div className="grid2">
          <label className="field">
            <span>Connexions par fichier</span>
            <input type="number" min={1} max={32} value={s.connections} onChange={num("connections")} />
          </label>
          <label className="field">
            <span>Téléchargements simultanés</span>
            <input type="number" min={1} max={10} value={s.maxConcurrent} onChange={num("maxConcurrent")} />
          </label>
          <label className="field">
            <span>Limite de débit (Ko/s, 0 = aucune)</span>
            <input type="number" min={0} step={128} value={s.speedLimitKbps} onChange={num("speedLimitKbps")} />
          </label>
          <label className="field">
            <span>Port du lien navigateur</span>
            <input type="number" value={s.serverPort} disabled title="Modifiable dans settings.json" />
          </label>
        </div>
        <div className="field">
          <span>Jeton de l'extension navigateur</span>
          <div className="row">
            <code className="path mono">{s.extensionToken}</code>
            <button
              type="button"
              className="btn"
              onClick={async () => {
                await navigator.clipboard.writeText(s.extensionToken);
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              }}
            >
              {copied ? "Copié" : "Copier"}
            </button>
          </div>
          <p className="hint">Collez ce jeton dans les options de l'extension Fleuve pour relier le navigateur.</p>
        </div>
        {error && <p className="error">{error}</p>}
        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Annuler
          </button>
          <button type="submit" className="btn btn--primary">
            Enregistrer
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** Navigateur d'après l'origine de l'extension qui demande l'appairage. */
function browserName(origin: string): string {
  if (origin.startsWith("moz-extension://")) return "Firefox";
  if (origin.startsWith("safari-web-extension://")) return "Safari";
  return "Chrome, Edge, Brave ou Arc";
}

/**
 * Une extension du navigateur demande à se connecter à Fleuve. L'utilisateur
 * compare le code avec celui affiché par l'extension avant d'accepter : sans
 * « Autoriser », rien n'est transmis. Fermer la fenêtre revient à refuser.
 */
export function PairingDialog({
  pairing,
  onDecide,
}: {
  pairing: PairingInfo;
  onDecide: (approve: boolean) => Promise<void>;
}) {
  const [left, setLeft] = useState(pairing.expiresIn);
  useEffect(() => {
    setLeft(pairing.expiresIn);
    const t = window.setInterval(() => setLeft((s) => Math.max(0, s - 1)), 1000);
    return () => window.clearInterval(t);
  }, [pairing.id, pairing.expiresIn]);
  return (
    <Modal title="Connecter l'extension du navigateur ?" onClose={() => void onDecide(false)}>
      <p className="hint">
        Une extension ({browserName(pairing.origin)}) demande à envoyer des téléchargements à Fleuve. Vérifiez que
        ce code est <b>le même</b> que celui affiché dans les options de l'extension :
      </p>
      <p className="pairing__code mono">{pairing.code}</p>
      <p className="hint">
        Si vous n'avez rien demandé, cliquez sur « Refuser ». Cette demande expire dans {left} s.
      </p>
      <div className="modal__actions">
        <button className="btn" onClick={() => void onDecide(false)}>
          Refuser
        </button>
        <button className="btn btn--primary" onClick={() => void onDecide(true)} disabled={left === 0}>
          Autoriser
        </button>
      </div>
    </Modal>
  );
}

export function ConfirmRemove({
  name,
  completed,
  onClose,
  onConfirm,
}: {
  name: string;
  completed: boolean;
  onClose: () => void;
  onConfirm: (deleteFile: boolean) => Promise<void>;
}) {
  const [del, setDel] = useState(false);
  return (
    <Modal title="Retirer de la liste ?" onClose={onClose}>
      <p className="hint">« {name} »</p>
      {completed && (
        <label className="check">
          <input type="checkbox" checked={del} onChange={(e) => setDel(e.target.checked)} />
          Supprimer aussi le fichier du disque
        </label>
      )}
      <div className="modal__actions">
        <button className="btn" onClick={onClose}>
          Annuler
        </button>
        <button
          className="btn btn--danger"
          onClick={async () => {
            await onConfirm(del);
            onClose();
          }}
          autoFocus
        >
          Retirer
        </button>
      </div>
    </Modal>
  );
}

export function ConfirmClear({
  finished,
  total,
  onClose,
  onConfirm,
}: {
  finished: number;
  total: number;
  onClose: () => void;
  onConfirm: (all: boolean) => Promise<void>;
}) {
  const [all, setAll] = useState(finished === 0);
  const unfinished = total - finished;
  return (
    <Modal title="Vider la liste ?" onClose={onClose}>
      <div className="form">
        <label className="check">
          <input type="radio" name="clear" checked={!all} disabled={finished === 0} onChange={() => setAll(false)} />
          Terminés et en échec seulement ({finished})
        </label>
        <label className="check">
          <input type="radio" name="clear" checked={all} onChange={() => setAll(true)} />
          Tout ({total})
        </label>
        <p className="hint">
          Les fichiers terminés restent sur le disque.
          {all && unfinished > 0
            ? ` Les ${unfinished} téléchargement(s) non terminé(s) seront arrêtés et leurs fichiers partiels supprimés.`
            : ""}
        </p>
      </div>
      <div className="modal__actions">
        <button className="btn" onClick={onClose}>
          Annuler
        </button>
        <button
          className="btn btn--danger"
          onClick={async () => {
            await onConfirm(all);
            onClose();
          }}
          autoFocus
        >
          Vider
        </button>
      </div>
    </Modal>
  );
}

/** Choix de la qualité d'une vidéo reçue (réglage « Demander à chaque fois »). */
export function QualityDialog({
  id,
  name,
  onClose,
}: {
  id: string;
  name: string;
  onClose: () => void;
}) {
  const [quality, setQuality] = useState("best");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useMemo(() => () => api.probeQualitiesFor(id), [id]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    try {
      await api.setQuality(id, quality);
      onClose();
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <Modal title="Choisir la qualité" onClose={onClose}>
      <form onSubmit={submit} className="form">
        <p className="ellipsis" title={name}>
          {name}
        </p>
        <label className="field">
          <span>Qualité vidéo</span>
          <QualitySelect load={load} value={quality} onChange={setQuality} />
        </label>
        {error && <p className="error">{error}</p>}
        <div className="modal__actions">
          <button type="button" className="btn" onClick={onClose}>
            Plus tard
          </button>
          <button type="submit" className="btn btn--primary" disabled={busy}>
            Télécharger
          </button>
        </div>
      </form>
    </Modal>
  );
}
