import type { Status } from "./types";

const UNITS = ["o", "Ko", "Mo", "Go", "To"];

/** Vrai sur macOS : raccourcis avec ⌘ et « Finder » ; ailleurs Ctrl et gestionnaire de fichiers. */
export const IS_MAC = /Mac/i.test(navigator.userAgent);
/** Vrai sous Linux : active le thème épuré inspiré de GNOME (voir styles.css). */
export const IS_LINUX = !IS_MAC && /Linux|X11/i.test(navigator.userAgent) && !/Android/i.test(navigator.userAgent);
/** Libellé de la touche de raccourci : « ⌘ » sur Mac, « Ctrl+ » ailleurs. */
export const MOD_KEY = IS_MAC ? "⌘" : "Ctrl+";

/** Taille lisible, en unités binaires comme le Finder affiche en français. */
export function formatBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  let v = n;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  const digits = u === 0 || v >= 100 ? 0 : 1;
  return `${v.toLocaleString("fr-FR", { maximumFractionDigits: digits, minimumFractionDigits: digits })} ${UNITS[u]}`;
}

export function formatSpeed(bps: number): string {
  return bps > 0 ? `${formatBytes(bps)}/s` : "—";
}

export function formatEta(secs: number | null): string {
  if (secs == null) return "—";
  if (secs < 60) return `${secs} s`;
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m} min ${String(secs % 60).padStart(2, "0")} s`;
  const h = Math.floor(m / 60);
  return `${h} h ${String(m % 60).padStart(2, "0")} min`;
}

export function formatDate(secs: number | null): string {
  if (secs == null) return "";
  return new Date(secs * 1000).toLocaleString("fr-FR", {
    day: "2-digit",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export const STATUS_LABEL: Record<Status, string> = {
  queued: "En file d'attente",
  scheduled: "Programmé",
  downloading: "En cours",
  assembling: "Assemblage",
  paused: "Arrêté",
  completed: "Terminé",
  failed: "Échec",
};

export const CATEGORIES = ["Vidéos", "Musique", "Documents", "Archives", "Programmes", "Images", "Autres"];

/** Couleur de la pastille d'extension, par catégorie. */
export const CATEGORY_COLOR: Record<string, string> = {
  Vidéos: "#7c4dff",
  Musique: "#e0457b",
  Documents: "#2f7de1",
  Archives: "#c98a12",
  Programmes: "#5b6b7f",
  Images: "#1f9d6b",
  Autres: "#8a8f98",
};

export function extensionOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 && i > name.length - 7 ? name.slice(i + 1).toUpperCase() : "";
}
