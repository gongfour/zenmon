// Theme resolution, same approach as dotori's launcher: the CSS knows only
// one `:root[data-theme="dark"]` block, and this module resolves
// "system|light|dark" into an actual light/dark value stamped onto
// documentElement — no duplicated @media definitions, and OS changes are
// followed live. Stored in localStorage: theme is a per-machine preference,
// not part of the app's config file.
export type ThemeMode = "system" | "light" | "dark";
export type Theme = "light" | "dark";

export const THEME_KEY = "zenmon-tray.theme";

/** Mode + OS dark preference → the theme actually applied. Pure. */
export const effectiveTheme = (mode: ThemeMode, systemDark: boolean): Theme =>
  mode === "system" ? (systemDark ? "dark" : "light") : mode;

/** Raw localStorage value → mode. Anything unrecognized falls back to system. */
export const normalizeMode = (raw: string | null): ThemeMode =>
  raw === "light" || raw === "dark" ? raw : "system";

export function loadMode(): ThemeMode {
  try {
    return normalizeMode(localStorage.getItem(THEME_KEY));
  } catch {
    return "system";
  }
}

const systemDark = () =>
  window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;

const apply = (mode: ThemeMode) => {
  document.documentElement.dataset.theme = effectiveTheme(mode, systemDark());
};

export function setThemeMode(mode: ThemeMode) {
  try {
    localStorage.setItem(THEME_KEY, mode);
  } catch {
    /* private mode etc. — still apply */
  }
  apply(mode);
}

/** Once at startup. Follows OS switches while in system mode. */
export function initTheme() {
  apply(loadMode());
  window.matchMedia?.("(prefers-color-scheme: dark)")
    .addEventListener?.("change", () => apply(loadMode()));
}
