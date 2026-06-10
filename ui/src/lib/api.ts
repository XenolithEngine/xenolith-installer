// Typed bridge to the Rust core via Tauri commands. Falls back to mock data when
// running in a plain browser (no Tauri), so the UI can be developed/previewed.
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Kind = "host" | "target";

export type Status =
  | { status: "not-installed" }
  | { status: "installed" }
  | { status: "update-available"; installed_release: string; latest_release: string };

export interface CatalogRow {
  id: string;
  triple: string;
  variant: string | null;
  kind: Kind;
  size: number;
  status: Status;
}

export interface Catalog {
  release: string;
  nativeId: string | null;
  rows: CatalogRow[];
  dropped: string[];
}

export type InstallPhase = "downloading" | "verifying" | "extracting" | "placing";

export interface InstallProgress {
  id: string;
  phase: InstallPhase;
  bytes: number;
}

const inTauri = (() => {
  try {
    return isTauri();
  } catch {
    return false;
  }
})();

export async function detectHost(): Promise<string> {
  if (!inTauri) return "aarch64-apple-macosx";
  return invoke<string>("detect_host");
}

export async function loadCatalog(): Promise<Catalog> {
  if (!inTauri) return mockCatalog();
  return invoke<Catalog>("load_catalog");
}

export async function install(id: string, kind: Kind): Promise<void> {
  if (!inTauri) {
    await new Promise((r) => setTimeout(r, 600));
    return;
  }
  await invoke("install_component", { id, kind });
}

export async function uninstall(id: string, kind: Kind): Promise<void> {
  if (!inTauri) {
    await new Promise((r) => setTimeout(r, 200));
    return;
  }
  await invoke("uninstall_component", { id, kind });
}

export async function onInstallProgress(
  cb: (p: InstallProgress) => void,
): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<InstallProgress>("install://progress", (e) => cb(e.payload));
}

export async function t(key: string, args?: Record<string, string>): Promise<string> {
  if (!inTauri) {
    let s = DEV_STRINGS[key] ?? key;
    if (args) for (const [k, v] of Object.entries(args)) s = s.replace(`{${k}}`, v);
    return s;
  }
  return invoke<string>("translate", { key, args: args ?? null });
}

// ---- dev fallbacks ----

const DEV_STRINGS: Record<string, string> = {
  "app-title": "Xenolith Installer",
  "group-hosts": "Development Tools",
  "group-targets": "Runtime Platforms",
  "status-installed": "Installed",
  "status-not-installed": "Not Installed",
  "status-update-available": "Update Available: {version}",
  "action-install": "Install",
  "action-refresh": "Refresh",
  "action-installing": "Installing…",
  "action-delete": "Delete",
  "action-cancel": "Cancel",
  "confirm-delete": "Delete {name}? This removes its installed files.",
  "phase-downloading": "Downloading",
  "phase-verifying": "Verifying",
  "phase-extracting": "Extracting",
  "phase-placing": "Finishing",
  loading: "Loading catalogue…",
  "col-name": "Name",
  "col-size": "Size",
  "col-status": "Status",
};

function mockCatalog(): Catalog {
  const mk = (id: string, kind: Kind, status: Status, variant: string | null = null): CatalogRow => ({
    id,
    triple: id.replace("+sprt", ""),
    variant,
    kind,
    size: 20_000_000,
    status,
  });
  return {
    release: "sdk-v0alpha0",
    nativeId: "aarch64-apple-macosx",
    dropped: [],
    rows: [
      mk("aarch64-apple-macosx", "host", { status: "installed" }),
      mk("x86_64-pc-windows-msvc", "host", { status: "not-installed" }),
      mk("x86_64-unknown-linux-gnu", "host", { status: "not-installed" }),
      mk("aarch64-apple-macosx", "target", { status: "not-installed" }),
      mk("aarch64-apple-macosx+sprt", "target", { status: "not-installed" }, "sprt"),
      mk("aarch64-unknown-linux-gnu", "target", {
        status: "update-available",
        installed_release: "sdk-v0alpha0prev",
        latest_release: "sdk-v0alpha0",
      }),
      mk("x86_64-unknown-linux-android", "target", { status: "not-installed" }),
    ],
  };
}
