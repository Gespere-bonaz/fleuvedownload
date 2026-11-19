import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { AddRequest, DownloadView, Settings } from "./types";
import { CATEGORIES, IS_MAC, MOD_KEY, CATEGORY_COLOR, STATUS_LABEL, extensionOf, formatBytes, formatDate, formatEta, formatSpeed } from "./format";
import { SegmentBar } from "./components/SegmentBar";
import { AddDialog, ConfirmClear, ConfirmRemove, ScheduleDialog, SettingsDialog } from "./components/Dialogs";

type Filter =
  | { kind: "all" }
  | { kind: "active" }
  | { kind: "waiting" }
  | { kind: "done" }
  | { kind: "failed" }
  | { kind: "category"; name: string };

const FILTERS: { key: string; label: string; f: Filter }[] = [
  { key: "all", label: "Tous", f: { kind: "all" } },
  { key: "active", label: "En cours", f: { kind: "active" } },
  { key: "waiting", label: "En attente", f: { kind: "waiting" } },
  { key: "done", label: "Terminés", f: { kind: "done" } },
  { key: "failed", label: "Échecs", f: { kind: "failed" } },
];

function matches(d: DownloadView, f: Filter): boolean {
  switch (f.kind) {
    case "all":
      return true;
    case "active":
      return d.status === "downloading" || d.status === "assembling";
    case "waiting":
      return d.status === "queued" || d.status === "scheduled" || d.status === "paused";
    case "done":
      return d.status === "completed";
    case "failed":
      return d.status === "failed";
    case "category":
      return d.category === f.name;
  }
}

function filterKey(f: Filter) {
  return f.kind === "category" ? `cat:${f.name}` : f.kind;
}

type DialogState =
  | null
  | { kind: "add"; url: string }
  | { kind: "settings" }
  | { kind: "schedule"; id: string }
  | { kind: "remove"; d: DownloadView }
  | { kind: "clear" };

export default function App() {
  const [items, setItems] = useState<DownloadView[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [filter, setFilter] = useState<Filter>({ kind: "all" });
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [toast, setToast] = useState<string | null>(null);

  const notify = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast((t) => (t === msg ? null : t)), 3500);
  }, []);

  useEffect(() => {
    api.list().then(setItems);
    api.getSettings().then(setSettings);
    const un = [
      listen<DownloadView[]>("downloads", (e) => setItems(e.payload)),
      listen<string>("added-from-browser", (e) => {
        setSelected(e.payload);
        notify("Téléchargement reçu du navigateur");
      }),
      listen<string>("server-error", (e) => notify(e.payload)),
    ];
    return () => un.forEach((p) => p.then((f) => f()));
  }, [notify]);

  // ⌘N (Ctrl+N hors Mac) : ajouter ; coller un lien n'importe où ouvre la fenêtre d'ajout.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (dialog) return;
      if ((IS_MAC ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === "n") {
        e.preventDefault();
        setDialog({ kind: "add", url: "" });
      }
    };
    const onPaste = (e: ClipboardEvent) => {
      if (dialog || (e.target as HTMLElement)?.tagName === "INPUT") return;
      const text = e.clipboardData?.getData("text") ?? "";
      if (/^https?:\/\//i.test(text.trim())) setDialog({ kind: "add", url: text.trim() });
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("paste", onPaste);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("paste", onPaste);
    };
  }, [dialog]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return items
      .filter((d) => matches(d, filter))
      .filter((d) => !q || d.fileName.toLowerCase().includes(q) || d.url.toLowerCase().includes(q))
      .sort((a, b) => b.createdAt - a.createdAt);
  }, [items, filter, query]);

  const current = items.find((d) => d.id === selected) ?? null;
  const totalSpeed = items.reduce((s, d) => s + d.speed, 0);
  const activeCount = items.filter((d) => matches(d, { kind: "active" })).length;

  const run = (p: Promise<unknown>) => p.catch((e) => notify(String(e)));

  const canPause = current && ["downloading", "assembling", "queued", "scheduled"].includes(current.status);
  const canResume = current && ["paused", "failed", "scheduled"].includes(current.status);
  const stoppable = (d: DownloadView) => ["downloading", "assembling", "queued", "scheduled"].includes(d.status);
  const anyStoppable = items.some(stoppable);
  const stopAll = () =>
    run(api.pauseAll().then((n) => n > 0 && notify(n > 1 ? `${n} téléchargements arrêtés` : "Téléchargement arrêté")));

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="sidebar__drag" data-tauri-drag-region />
        <nav>
          {FILTERS.map(({ key, label, f }) => (
            <button key={key} className={`navitem ${filterKey(filter) === key ? "on" : ""}`} onClick={() => setFilter(f)}>
              <span>{label}</span>
              <span className="count">{items.filter((d) => matches(d, f)).length || ""}</span>
            </button>
          ))}
          <div className="navtitle">Catégories</div>
          {CATEGORIES.map((c) => {
            const f: Filter = { kind: "category", name: c };
            const n = items.filter((d) => d.category === c).length;
            return (
              <button key={c} className={`navitem ${filterKey(filter) === filterKey(f) ? "on" : ""}`} onClick={() => setFilter(f)}>
                <span>
                  <i className="dot" style={{ background: CATEGORY_COLOR[c] }} />
                  {c}
                </span>
                <span className="count">{n || ""}</span>
              </button>
            );
          })}
        </nav>
        <div className="sidebar__foot">
          {activeCount > 0 ? (
            <>
              <strong>{formatSpeed(totalSpeed)}</strong>
              <span>
                {activeCount} en cours
                {settings && settings.speedLimitKbps > 0 ? ` · limite ${formatBytes(settings.speedLimitKbps * 1024)}/s` : ""}
              </span>
            </>
          ) : (
            <span>Aucun téléchargement actif</span>
          )}
        </div>
      </aside>

      <main className="main">
        <header className="toolbar" data-tauri-drag-region>
          <button className="btn btn--primary" onClick={() => setDialog({ kind: "add", url: "" })} title={`Ajouter (${MOD_KEY}N)`}>
            + Ajouter
          </button>
          <div className="toolbar__group">
            <button className="btn" disabled={!canResume} onClick={() => current && run(api.resume(current.id))}>
              Reprendre
            </button>
            <button className="btn" disabled={!canPause} title="Arrêter (la progression est gardée)" onClick={() => current && run(api.pause(current.id))}>
              Arrêter
            </button>
            <button className="btn" disabled={!anyStoppable} onClick={stopAll}>
              Tout arrêter
            </button>
            <button
              className="btn btn--hide-narrow"
              disabled={!current || current.status === "completed"}
              onClick={() => current && setDialog({ kind: "schedule", id: current.id })}
            >
              Programmer
            </button>
            <button className="btn" disabled={!current} onClick={() => current && setDialog({ kind: "remove", d: current })}>
              Retirer
            </button>
            <button className="btn" disabled={items.length === 0} title="Retirer plusieurs téléchargements de la liste" onClick={() => setDialog({ kind: "clear" })}>
              Vider…
            </button>
          </div>
          <div className="toolbar__group">
            <button className="btn" disabled={current?.status !== "completed"} onClick={() => current && run(api.open(current.id))}>
              Ouvrir
            </button>
            <button className="btn btn--hide-narrow" disabled={!current} title={IS_MAC ? "Afficher dans le Finder" : "Afficher dans le dossier"} onClick={() => current && run(api.reveal(current.id))}>
              {IS_MAC ? "Finder" : "Dossier"}
            </button>
          </div>
          <div className="toolbar__spacer" data-tauri-drag-region />
          <input className="search" type="search" placeholder="Rechercher" value={query} onChange={(e) => setQuery(e.target.value)} />
          <button className="btn btn--icon" title="Réglages" onClick={() => setDialog({ kind: "settings" })} aria-label="Réglages">
            ⚙
          </button>
        </header>

        <section className="list" onClick={(e) => e.target === e.currentTarget && setSelected(null)}>
          {visible.length === 0 ? (
            <div className="empty">
              <p className="empty__title">{items.length === 0 ? "Aucun téléchargement" : "Rien dans cette vue"}</p>
              {items.length === 0 && (
                <p>
                  Cliquez sur « Ajouter », collez un lien ({MOD_KEY}V) ou utilisez l'extension du navigateur.
                </p>
              )}
            </div>
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th className="col-name">Nom</th>
                  <th className="col-size">Taille</th>
                  <th className="col-progress">Progression</th>
                  <th className="col-speed">Vitesse</th>
                  <th className="col-eta">Restant</th>
                  <th className="col-status">Statut</th>
                  <th className="col-act" aria-label="Actions" />
                </tr>
              </thead>
              <tbody>
                {visible.map((d) => (
                  <tr
                    key={d.id}
                    className={d.id === selected ? "sel" : ""}
                    onClick={() => setSelected(d.id)}
                    onDoubleClick={() => d.status === "completed" && run(api.open(d.id))}
                  >
                    <td className="col-name">
                      <div className="namecell">
                        <span className="badge" style={{ background: CATEGORY_COLOR[d.category] ?? CATEGORY_COLOR.Autres }}>
                          {extensionOf(d.fileName) || "?"}
                        </span>
                        <span className="name" title={d.fileName}>
                          {d.fileName}
                        </span>
                      </div>
                    </td>
                    <td className="col-size num">
                      {d.status === "completed"
                        ? formatBytes(d.total ?? d.received)
                        : d.total
                          ? formatBytes(d.total)
                          : d.received > 0
                            ? formatBytes(d.received)
                            : "—"}
                    </td>
                    <td className="col-progress">
                      <SegmentBar d={d} />
                      <span className="pct num">{d.progress != null ? `${Math.floor(d.progress * 100)} %` : ""}</span>
                    </td>
                    <td className="col-speed num">{formatSpeed(d.speed)}</td>
                    <td className="col-eta num">{formatEta(d.etaSecs)}</td>
                    <td className={`col-status st-${d.status}`}>
                      {d.status === "scheduled" ? `Programmé ${formatDate(d.scheduledAt)}` : STATUS_LABEL[d.status]}
                    </td>
                    <td className="col-act">
                      {stoppable(d) ? (
                        <button
                          className="rowbtn"
                          title="Arrêter"
                          aria-label="Arrêter"
                          onClick={(e) => {
                            e.stopPropagation();
                            run(api.pause(d.id));
                          }}
                        >
                          ■
                        </button>
                      ) : d.status === "paused" || d.status === "failed" ? (
                        <button
                          className="rowbtn"
                          title="Reprendre"
                          aria-label="Reprendre"
                          onClick={(e) => {
                            e.stopPropagation();
                            run(api.resume(d.id));
                          }}
                        >
                          ▶
                        </button>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        {current && (
          <section className="detail">
            <div className="detail__head">
              <strong title={current.fileName}>{current.fileName}</strong>
              <span className={`st-${current.status}`}>{STATUS_LABEL[current.status]}</span>
            </div>
            <SegmentBar d={current} tall />
            <dl className="detail__grid">
              <dt>Reçu</dt>
              <dd className="num">
                {formatBytes(current.received)}
                {current.total ? ` sur ${formatBytes(current.total)}` : ""}
              </dd>
              <dt>{current.kind === "hls" ? "Morceaux" : "Connexions"}</dt>
              <dd className="num">
                {current.kind === "hls"
                  ? `${current.hlsDoneParts} / ${current.hlsTotalParts || "?"} · ${current.connections} en parallèle`
                  : `${current.connections} actives · ${current.segments.length} segments`}
              </dd>
              <dt>Reprise</dt>
              <dd>{current.resumable ? "Possible" : current.status === "completed" ? "—" : "Impossible (le serveur refuse les plages)"}</dd>
              <dt>Dossier</dt>
              <dd className="ellipsis" title={current.destDir}>
                {current.destDir}
              </dd>
              <dt>Lien</dt>
              <dd className="ellipsis mono" title={current.url}>
                {current.url}
              </dd>
            </dl>
            {current.error && <p className="error">{current.error}</p>}
          </section>
        )}
      </main>

      {dialog?.kind === "add" && settings && (
        <AddDialog
          initialUrl={dialog.url}
          defaultDir={settings.downloadDir}
          onClose={() => setDialog(null)}
          onAdd={async (reqs: AddRequest[]) => {
            const ids = await api.add(reqs);
            if (ids.length === 1) setSelected(ids[0]);
          }}
        />
      )}
      {dialog?.kind === "settings" && settings && (
        <SettingsDialog
          initial={settings}
          onClose={() => setDialog(null)}
          onSave={async (s) => setSettings(await api.saveSettings(s))}
        />
      )}
      {dialog?.kind === "schedule" && (
        <ScheduleDialog onClose={() => setDialog(null)} onSchedule={(at) => api.schedule(dialog.id, at)} />
      )}
      {dialog?.kind === "remove" && (
        <ConfirmRemove
          name={dialog.d.fileName}
          completed={dialog.d.status === "completed"}
          onClose={() => setDialog(null)}
          onConfirm={async (del) => {
            await api.remove(dialog.d.id, del);
            setSelected(null);
          }}
        />
      )}
      {dialog?.kind === "clear" && (
        <ConfirmClear
          finished={items.filter((d) => d.status === "completed" || d.status === "failed").length}
          total={items.length}
          onClose={() => setDialog(null)}
          onConfirm={async (all) => {
            const n = await api.clearList(all);
            setSelected(null);
            notify(`${n} téléchargement(s) retiré(s) de la liste`);
          }}
        />
      )}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
