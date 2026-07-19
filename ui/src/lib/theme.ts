/**
 * Three-state theme (system/light/dark), frontend-only per issue #36 D2:
 * persisted in localStorage, never part of the Rust settings schema. The
 * no-flash boot script in index.html reads the same key before first paint.
 */

export type Theme = "system" | "light" | "dark";

const STORAGE_KEY = "crfty-theme";

export function getTheme(): Theme {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
}

export function setTheme(theme: Theme): void {
  if (theme === "system") {
    localStorage.removeItem(STORAGE_KEY);
  } else {
    localStorage.setItem(STORAGE_KEY, theme);
  }
  applyTheme(theme);
}

export function applyTheme(theme: Theme = getTheme()): void {
  const dark =
    theme === "dark" || (theme === "system" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

/** Follow OS theme changes while in "system" mode. Returns an unsubscribe. */
export function watchSystemTheme(): () => void {
  const mq = matchMedia("(prefers-color-scheme: dark)");
  const onChange = () => {
    if (getTheme() === "system") applyTheme("system");
  };
  mq.addEventListener("change", onChange);
  return () => mq.removeEventListener("change", onChange);
}
