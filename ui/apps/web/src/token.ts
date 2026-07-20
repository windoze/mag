const TOKEN_STORAGE_KEY = "mag.web.token";

let memoryToken: string | undefined;

/** Minimal storage surface used to persist the per-tab web auth token. */
export interface ShellStorage {
  readonly getItem: (key: string) => string | null;
  readonly setItem: (key: string, value: string) => void;
  readonly removeItem: (key: string) => void;
}

/** Minimal location surface used to read `#t=` token fragments. */
export interface ShellLocation {
  readonly hash: string;
  readonly pathname: string;
  readonly search: string;
}

/** Minimal history surface used to clear captured token fragments. */
export interface ShellHistory {
  readonly replaceState: (data: unknown, unused: string, url?: string | URL | null) => void;
}

/** Captures `#t=<token>` into session storage and removes the token from the URL. */
export function captureFragmentToken(
  storage = getSessionStorage(),
  location = getWindowLocation(),
  history = getWindowHistory()
): string | undefined {
  if (location === undefined) {
    return readStoredToken(storage);
  }

  const fragment = location.hash.startsWith("#") ? location.hash.slice(1) : location.hash;
  if (fragment.length === 0) {
    return readStoredToken(storage);
  }

  const params = new URLSearchParams(fragment);
  const token = params.get("t") ?? undefined;
  if (token === undefined || token.length === 0) {
    return readStoredToken(storage);
  }

  writeStoredToken(token, storage);
  params.delete("t");
  const nextFragment = params.toString();
  history?.replaceState(
    null,
    "",
    `${location.pathname}${location.search}${nextFragment.length === 0 ? "" : `#${nextFragment}`}`
  );
  return token;
}

/** Reads the captured token, falling back to in-memory storage when sessionStorage is blocked. */
export function readStoredToken(storage = getSessionStorage()): string | undefined {
  try {
    return storage?.getItem(TOKEN_STORAGE_KEY) ?? memoryToken;
  } catch {
    return memoryToken;
  }
}

function writeStoredToken(token: string, storage: ShellStorage | undefined): void {
  memoryToken = token;
  try {
    storage?.setItem(TOKEN_STORAGE_KEY, token);
  } catch {
    // Some locked-down browser contexts deny sessionStorage; keep the memory token for this tab.
  }
}

function getSessionStorage(): ShellStorage | undefined {
  try {
    return globalThis.sessionStorage;
  } catch {
    return undefined;
  }
}

function getWindowLocation(): ShellLocation | undefined {
  return typeof window === "undefined" ? undefined : window.location;
}

function getWindowHistory(): ShellHistory | undefined {
  return typeof window === "undefined" ? undefined : window.history;
}
