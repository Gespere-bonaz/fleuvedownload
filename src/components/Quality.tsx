import { useEffect, useState } from "react";
import { formatBytes } from "../format";
import type { QualityOption } from "../types";

/** Hauteurs proposées quand le site ne dit pas lesquelles il a. */
export const STANDARD_HEIGHTS = [2160, 1440, 1080, 720, 480, 360];

/** Libellé d'un réglage de qualité : « best » → « Meilleure », « 720 » → « 720p ». */
export function qualityLabel(q: string | number | null | undefined): string {
  if (q == null || q === "best" || q === "") return "Meilleure";
  if (q === "ask") return "Demander";
  return `${q}p`;
}

/** Valeur de départ du sélecteur, d'après le réglage (« ask » → la meilleure). */
export function initialQuality(setting: string | undefined): string {
  return !setting || setting === "ask" ? "best" : setting;
}

/**
 * Liste déroulante des qualités d'une vidéo. Détecte les qualités réellement
 * proposées par le site ; sans réponse, propose les hauteurs courantes.
 */
export function QualitySelect({
  load,
  value,
  onChange,
}: {
  /** Détection des qualités ; `null` pour ne proposer que les hauteurs courantes. */
  load: (() => Promise<QualityOption[]>) | null;
  value: string;
  onChange: (v: string) => void;
}) {
  const [options, setOptions] = useState<QualityOption[] | null>(load ? null : []);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!load) return;
    let alive = true;
    setOptions(null);
    setError(null);
    load()
      .then((o) => alive && setOptions(o))
      .catch((e) => {
        if (!alive) return;
        setError(String(e));
        setOptions([]);
      });
    return () => {
      alive = false;
    };
  }, [load]);

  const loading = options === null;
  const detected = options && options.length > 0;
  const list: QualityOption[] = detected ? options : STANDARD_HEIGHTS.map((height) => ({ height, size: null }));
  const best = detected ? options[0] : null;
  const missing = value !== "best" && !list.some((o) => String(o.height) === value);

  return (
    <>
      <select value={value} onChange={(e) => onChange(e.target.value)} disabled={loading}>
        {loading ? (
          <option value={value}>Recherche des qualités disponibles…</option>
        ) : (
          <>
            <option value="best">
              Meilleure disponible{best ? ` (${best.height}p${best.size ? ` · ${formatBytes(best.size)}` : ""})` : ""}
            </option>
            {list.map((o) => (
              <option key={o.height} value={String(o.height)}>
                {detected ? `${o.height}p` : `${o.height}p au plus`}
                {o.size ? ` · ${formatBytes(o.size)}` : ""}
              </option>
            ))}
            {missing && <option value={value}>{value}p au plus</option>}
          </>
        )}
      </select>
      {!loading && !detected && load && (
        <span className="hint">{error ? `Qualités non détectées : ${error}` : "Une seule qualité annoncée par le site."}</span>
      )}
    </>
  );
}
