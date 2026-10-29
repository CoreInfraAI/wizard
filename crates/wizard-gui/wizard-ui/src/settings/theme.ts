import { getCurrentWindow } from "@tauri-apps/api/window";
import { reportError } from "../logs/log";

export type Theme = "light" | "dark" | "system";

const key = "coreinfra.theme";
const listeners = new Set<() => void>();
let theme: Theme = "system";
let initialized = false;
const isTheme = (value: unknown): value is Theme =>
  value === "light" || value === "dark" || value === "system";

export const getTheme = () => theme;
export function subscribeTheme(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function applyTheme() {
  document.documentElement.dataset.theme = theme === "system"
    ? window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
    : theme;
  void getCurrentWindow().setTheme(theme === "system" ? null : theme)
    .catch((cause) => reportError("Не удалось изменить тему окна", cause));
  listeners.forEach((listener) => listener());
}

export function setTheme(value: Theme) {
  theme = value;
  try { localStorage.setItem(key, value); } catch { /* Apply even without storage. */ }
  applyTheme();
}

export function initializeTheme() {
  if (initialized) return;
  initialized = true;
  try {
    const saved = localStorage.getItem(key);
    if (isTheme(saved)) theme = saved;
  } catch { /* Use the system theme when storage is unavailable. */ }
  applyTheme();
  window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (theme === "system") applyTheme();
  });
  window.addEventListener("storage", (event) => {
    if (event.key !== key && event.key !== null) return;
    theme = isTheme(event.newValue) ? event.newValue : "system";
    applyTheme();
  });
}
