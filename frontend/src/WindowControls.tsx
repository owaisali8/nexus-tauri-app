import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

export function WindowControls() {
  if (!isTauri()) {
    return null;
  }

  const appWindow = getCurrentWindow();

  return (
    <div className="window-controls">
      <button
        type="button"
        className="window-control"
        aria-label="Minimize"
        onClick={() => void appWindow.minimize()}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M1 5.5h8" stroke="currentColor" strokeWidth="1.25" />
        </svg>
      </button>
      <button
        type="button"
        className="window-control"
        aria-label="Maximize"
        onClick={() => void appWindow.toggleMaximize()}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="1.25" y="1.25" width="7.5" height="7.5" stroke="currentColor" strokeWidth="1.25" fill="none" />
        </svg>
      </button>
      <button
        type="button"
        className="window-control close"
        aria-label="Close"
        onClick={() => void appWindow.close()}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" strokeWidth="1.25" />
        </svg>
      </button>
    </div>
  );
}
