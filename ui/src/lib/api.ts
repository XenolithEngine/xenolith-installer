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
  kind: "host" | "target";
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

export interface EngineInfo {
  reference: string;
  short: string;
}

export interface EngineProgress {
  phase: "downloading" | "done";
  bytes: number;
}

export async function engineStatus(): Promise<EngineInfo | null> {
  if (!inTauri) return { reference: "master", short: "devbuild" };
  return invoke<EngineInfo | null>("engine_status");
}

export async function prepareEngine(): Promise<EngineInfo> {
  if (!inTauri) {
    await new Promise((r) => setTimeout(r, 800));
    return { reference: "master", short: "devbuild" };
  }
  return invoke<EngineInfo>("prepare_engine");
}

export async function onEngineProgress(
  cb: (p: EngineProgress) => void,
): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<EngineProgress>("engine://progress", (e) => cb(e.payload));
}

export async function onInstallProgress(
  cb: (p: InstallProgress) => void,
): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<InstallProgress>("install://progress", (e) => cb(e.payload));
}

// ---- projects ----

export interface Project {
  name: string;
  path: string;
  engine: string;
  target: string;
  createdAt: string;
}

export async function projectEngines(): Promise<string[]> {
  if (!inTauri) return ["master"];
  return invoke<string[]>("project_engines");
}

export interface Targets {
  targets: string[];
  host: string;
  hostInstalled: boolean;
}

export async function projectTargets(): Promise<Targets> {
  if (!inTauri) {
    return { targets: ["aarch64-apple-macosx"], host: "aarch64-apple-macosx", hostInstalled: true };
  }
  return invoke<Targets>("project_targets");
}

export async function listProjects(): Promise<Project[]> {
  if (!inTauri) return [];
  return invoke<Project[]>("list_projects");
}

export async function createProject(
  location: string,
  name: string,
  engine: string,
  target: string,
): Promise<Project> {
  if (!inTauri) {
    return { name, path: `${location}/${name}`, engine, target, createdAt: "now" };
  }
  return invoke<Project>("create_project", { location, name, engine, target });
}

export async function removeProject(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("remove_project", { path });
}

export interface Editor {
  id: string;
  name: string;
}

export async function availableEditors(): Promise<Editor[]> {
  if (!inTauri) {
    return [
      { id: "files", name: "Finder" },
      { id: "vscode", name: "VS Code" },
      { id: "claude", name: "Claude Code" },
    ];
  }
  return invoke<Editor[]>("available_editors");
}

export async function openInEditor(path: string, editor: string): Promise<void> {
  if (!inTauri) return;
  await invoke("open_in_editor", { path, editor });
}

export async function openWorkingDir(): Promise<void> {
  if (!inTauri) return;
  await invoke("open_working_dir");
}

export async function openTerminal(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("open_terminal", { path });
}

export async function cleanProject(path: string): Promise<void> {
  if (!inTauri) return;
  await invoke("clean_project", { path });
}

export async function installForSystem(): Promise<void> {
  if (!inTauri) {
    await new Promise((r) => setTimeout(r, 600));
    return;
  }
  await invoke("install_for_system");
}

export interface StorageItem {
  id: string;
  bytes: number;
}
export interface Storage {
  engines: StorageItem[];
  hosts: StorageItem[];
  targets: StorageItem[];
  total: number;
}

export async function diskUsage(): Promise<Storage> {
  if (!inTauri) {
    return {
      engines: [{ id: "master", bytes: 420_000_000 }],
      hosts: [{ id: "aarch64-apple-macosx", bytes: 310_000_000 }],
      targets: [{ id: "aarch64-apple-macosx", bytes: 120_000_000 }],
      total: 850_000_000,
    };
  }
  return invoke<Storage>("disk_usage");
}

export async function removeEngine(reference: string): Promise<void> {
  if (!inTauri) return;
  await invoke("remove_engine", { reference });
}

export async function projectSize(path: string): Promise<number> {
  if (!inTauri) return 0;
  return invoke<number>("project_size", { path });
}

export async function buildProject(
  path: string,
  target: string,
  run: boolean,
): Promise<number> {
  if (!inTauri) return 0;
  return invoke<number>("build_project", { path, target, run });
}

export async function onBuildLine(cb: (line: string) => void): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  return listen<{ line: string }>("build://line", (e) => cb(e.payload.line));
}

export async function cancelBuild(): Promise<void> {
  if (!inTauri) return;
  await invoke("cancel_build");
}

export async function diagnosticsReport(): Promise<string> {
  if (!inTauri) return "Xenolith Installer (dev) — no diagnostics in browser preview";
  return invoke<string>("diagnostics_report");
}

export interface DoctorCheck {
  name: string;
  ok: boolean;
  detail: string;
}
export async function runDoctor(): Promise<DoctorCheck[]> {
  if (!inTauri) {
    return [
      { name: "Engine present", ok: true, detail: "~/.local/share/xenolith" },
      { name: "Host toolchain", ok: true, detail: "aarch64-apple-macosx" },
    ];
  }
  return invoke<DoctorCheck[]>("run_doctor");
}

/// Native folder picker (Tauri dialog plugin). Returns the chosen path or null.
export async function pickFolder(): Promise<string | null> {
  if (!inTauri) return null;
  const { open } = await import("@tauri-apps/plugin-dialog");
  const res = await open({ directory: true, multiple: false });
  return typeof res === "string" ? res : null;
}

// Active UI language ("en"/"ru"); empty = follow the system locale. Set from
// the persisted setting at startup and on toggle, then passed to every lookup.
let currentLang = "";
export function setLang(lang: string | null) {
  currentLang = lang ?? "";
}

export async function t(key: string, args?: Record<string, string>): Promise<string> {
  if (!inTauri) {
    let s = DEV_STRINGS[key] ?? key;
    if (args) for (const [k, v] of Object.entries(args)) s = s.replace(`{${k}}`, v);
    return s;
  }
  return invoke<string>("translate", { key, args: args ?? null, lang: currentLang || null });
}

export interface AppSettings {
  language: string | null;
  jobs: number | null;
  autoJobs: number;
  dataDir: string;
  defaultDataDir: string;
  dataDirOverride: string | null;
}

export async function getSettings(): Promise<AppSettings> {
  if (!inTauri) {
    return {
      language: null,
      jobs: null,
      autoJobs: 8,
      dataDir: "~/.local/share/xenolith",
      defaultDataDir: "~/.local/share/xenolith",
      dataDirOverride: null,
    };
  }
  return invoke<AppSettings>("get_settings");
}

export async function setSettings(language: string | null, jobs: number | null): Promise<void> {
  if (!inTauri) return;
  await invoke("set_settings", { language, jobs });
}

export async function setDataDir(path: string | null): Promise<void> {
  if (!inTauri) return;
  await invoke("set_data_dir", { path });
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
