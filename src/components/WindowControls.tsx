import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

const win = getCurrentWindow();

/**
 * Boutons de fenêtre intégrés à la barre d'outils (Linux, fenêtre sans
 * barre de titre native). Pose aussi la classe « maximized » sur <html>
 * pour retirer les coins arrondis quand la fenêtre occupe tout l'écran.
 */
export function WindowControls() {
  useEffect(() => {
    const sync = () => win.isMaximized().then((m) => document.documentElement.classList.toggle("maximized", m)).catch(() => {});
    sync();
    const off = win.onResized(sync);
    return () => {
      off.then((f) => f());
    };
  }, []);

  return (
    <div className="winctl">
      <button className="winctl__btn" title="Réduire" aria-label="Réduire" onClick={() => win.minimize()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 8.5h8" /></svg>
      </button>
      <button className="winctl__btn" title="Agrandir" aria-label="Agrandir" onClick={() => win.toggleMaximize()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="4.5" y="4.5" width="7" height="7" rx="1" /></svg>
      </button>
      <button className="winctl__btn" title="Fermer" aria-label="Fermer" onClick={() => win.close()}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 5l6 6M11 5l-6 6" /></svg>
      </button>
    </div>
  );
}
