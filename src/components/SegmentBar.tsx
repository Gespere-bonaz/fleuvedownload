import type { DownloadView } from "../types";

/**
 * Barre de progression façon IDM : chaque segment montre sa partie reçue
 * (bleu), un trait rose marque le point de départ de chaque connexion,
 * le blanc reste à télécharger.
 */
export function SegmentBar({ d, tall = false }: { d: DownloadView; tall?: boolean }) {
  const total = d.total ?? 0;
  const cls = `segbar ${tall ? "segbar--tall" : ""} segbar--${d.status}`;

  // Flux HLS ou taille inconnue : barre simple.
  if (d.kind === "hls" || total <= 0 || d.segments.length === 0) {
    const p = d.progress ?? (d.status === "completed" ? 1 : 0);
    return (
      <div className={cls} role="progressbar" aria-valuenow={Math.round(p * 100)} aria-valuemin={0} aria-valuemax={100}>
        <div className="segbar__fill" style={{ left: 0, width: `${p * 100}%` }} />
        {d.progress == null && d.status === "downloading" && <div className="segbar__indeterminate" />}
      </div>
    );
  }

  return (
    <div className={cls} role="progressbar" aria-valuenow={Math.round((d.progress ?? 0) * 100)} aria-valuemin={0} aria-valuemax={100}>
      {d.segments.map((s) => {
        const left = (s.start / total) * 100;
        const width = ((s.pos - s.start) / total) * 100;
        return (
          <div key={s.start}>
            <div className="segbar__fill" style={{ left: `${left}%`, width: `${width}%` }} />
            {tall && s.start > 0 && <div className="segbar__start" style={{ left: `${left}%` }} />}
            {tall && s.active && s.pos < s.end && <div className="segbar__head" style={{ left: `${(s.pos / total) * 100}%` }} />}
          </div>
        );
      })}
    </div>
  );
}
