<script lang="ts">
  import { onMount } from "svelte";
  import {
    loadCatalog,
    install,
    uninstall,
    onInstallProgress,
    engineStatus,
    prepareEngine,
    onEngineProgress,
    openWorkingDir,
    installForSystem,
    diskUsage,
    removeEngine,
    getSettings,
    setSettings,
    setDataDir,
    setLang,
    pickFolder,
    diagnosticsReport,
    runDoctor,
    projectTargets,
    t,
    type AppSettings,
    type DoctorCheck,
    type Catalog,
    type CatalogRow,
    type InstallProgress,
    type EngineInfo,
    type Kind,
    type Storage,
  } from "./lib/api";
  import Projects from "./lib/Projects.svelte";
  import logoUrl from "./assets/logo.webp";

  // English defaults so the UI never flashes raw keys; loadStrings() localises.
  const DEFAULTS: Record<string, string> = {
    "group-engine": "Engine",
    "group-hosts": "Development Tools",
    "group-targets": "Runtime Platforms",
    "status-installed": "Installed",
    "status-not-installed": "Not Installed",
    "action-install": "Install",
    "action-refresh": "Refresh",
    "action-installing": "Installing…",
    "action-delete": "Delete",
    "action-cancel": "Cancel",
    "engine-label": "Engine",
    "engine-missing": "Engine SDK not installed yet",
    "engine-prepare": "Prepare SDK",
    "engine-preparing": "Preparing SDK…",
    "engine-update": "Update engine (re-download)",
    "open-working-dir": "Open working directory",
    "nav-packages": "Packages",
    "nav-projects": "Projects",
    "project-new": "New Project",
    "project-name": "Project name",
    "project-name-rule": "Letters, digits, - and _ (no spaces)",
    "project-folder": "Location",
    "project-location": "Location (parent folder)",
    "project-choose": "Choose…",
    "project-engine": "Engine version",
    "project-target": "Build target",
    "run-host-only": "Run is only available when the target matches your host",
    "project-create": "Create",
    "project-build": "Build",
    "project-run": "Run",
    "project-open": "Open in",
    "project-remove": "Remove",
    "project-terminal": "Terminal",
    "project-clean": "Clean",
    "project-rebuild": "Rebuild",
    "projects-empty": "No projects yet",
    "install-all": "Install everything for my system",
    "install-all-hint": "Engine + your host toolchain + the native target",
    "action-repair": "Repair (re-download)",
    "storage-title": "Disk usage",
    "storage-engines": "Engines",
    "storage-hosts": "Host toolchains",
    "storage-targets": "Targets",
    "storage-total": "Total",
    "hero-sub": "The cross-platform engine SDK. Get set up in one click.",
    "hero-done": "All set!",
    "hero-create": "Create your first project",
    "hero-retry": "Try again",
    "hero-step-engine": "Engine",
    "hero-step-host": "Host toolchain",
    "hero-step-target": "Targets",
    "hero-manual": "Detailed setup",
    "hero-preview": "Onboarding (preview)",
    "doctor-title": "Check installation",
    "report-copy": "Copy diagnostics",
    "report-copied": "Diagnostics copied to clipboard",
    "settings-title": "Settings",
    "settings-language": "Language",
    "settings-auto": "Auto",
    "settings-jobs": "Parallel build jobs",
    "settings-datadir": "Data directory",
    "settings-reset": "Reset",
    "settings-restart": "Changing the data directory takes effect after a restart.",
    "datadir-no-space": "Path must not contain spaces",
    "path-no-space": "Path must not contain spaces",
    "project-choose": "Choose…",
    "engine-required": "Install the engine SDK and a host toolchain first",
    "create-requirements": "Install the engine, your host toolchain and a target (in Packages) before creating a project",
    "go-packages": "Go to Packages",
    "build-building": "Building…",
    loading: "Loading catalogue…",
    "col-name": "Name",
    "col-size": "Size",
    "col-status": "Status",
    "phase-downloading": "Downloading",
    "phase-verifying": "Verifying",
    "phase-extracting": "Extracting",
    "phase-placing": "Finishing",
  };

  let S = $state<Record<string, string>>({ ...DEFAULTS });
  let catalog = $state<Catalog | null>(null);
  let updateLabels = $state<Record<string, string>>({});
  let loading = $state(true);
  let error = $state<string | null>(null);
  let selected = $state<Record<string, boolean>>({});
  let collapsed = $state<Record<Kind, boolean>>({ target: false, host: false });
  let engineCollapsed = $state(false);
  let progress = $state<Record<string, InstallProgress>>({});
  let busy = $state(false);
  let engine = $state<EngineInfo | null>(null);
  let enginePrep = $state<number | null>(null); // bytes downloaded, or null
  let enginePrepTotal = $state(0); // total size for the re-download (0 = unknown)
  let tab = $state<"packages" | "projects">("packages");
  let settingsOpen = $state(false);

  // Engine first (rendered separately), then dev tools (hosts), then runtime
  // platforms (targets) — one consistent top-to-bottom order.
  const GROUP_ORDER: Kind[] = ["host", "target"];
  const groupTitle = (k: Kind) => (k === "target" ? S["group-targets"] : S["group-hosts"]);
  const key = (r: CatalogRow) => `${r.kind}:${r.id}`;
  const displayName = (r: CatalogRow) => (r.variant ? `${r.triple} +${r.variant}` : r.triple);

  const isNative = (r: CatalogRow) => !!catalog && r.triple === catalog.nativeId;

  function rowsFor(kind: Kind): CatalogRow[] {
    if (!catalog) return [];
    return catalog.rows
      .filter((r) => r.kind === kind)
      .sort((a, b) => Number(!isNative(a)) - Number(!isNative(b)));
  }

  const selectedRows = $derived(catalog ? catalog.rows.filter((r) => selected[key(r)]) : []);
  const selectedCount = $derived(selectedRows.length);

  const fmtSize = (b: number) => `${(b / 1_000_000).toFixed(1)} MB`;

  function pct(p: InstallProgress, size: number): number {
    if (p.phase === "downloading") return size > 0 ? Math.min(100, (p.bytes / size) * 100) : 0;
    return 100;
  }
  function pLabel(p: InstallProgress, size: number): string {
    if (p.phase === "downloading") return `${Math.round(pct(p, size))}%`;
    return S[`phase-${p.phase}`] ?? p.phase;
  }

  function statusText(r: CatalogRow): string {
    if (r.status.status === "installed") return S["status-installed"];
    if (r.status.status === "update-available") return updateLabels[key(r)] ?? "";
    return S["status-not-installed"];
  }

  async function loadStrings() {
    const keys = Object.keys(DEFAULTS);
    const entries = await Promise.all(keys.map(async (k) => [k, await t(k)] as const));
    S = { ...S, ...Object.fromEntries(entries) };
  }

  // ---- settings ----
  let settingsModal = $state(false);
  let appSettings = $state<AppSettings | null>(null);
  let jobsInput = $state<string>("");

  async function loadSettings() {
    appSettings = await getSettings();
    setLang(appSettings.language);
    jobsInput = appSettings.jobs != null ? String(appSettings.jobs) : "";
  }
  async function openSettings() {
    settingsOpen = false;
    appSettings = await getSettings();
    jobsInput = appSettings.jobs != null ? String(appSettings.jobs) : "";
    settingsModal = true;
  }
  async function chooseLanguage(lang: string | null) {
    if (!appSettings) return;
    appSettings = { ...appSettings, language: lang };
    setLang(lang);
    await setSettings(lang, appSettings.jobs);
    await loadStrings(); // re-fetch all strings in the new language
  }
  async function applyJobs() {
    if (!appSettings) return;
    const n = jobsInput.trim() === "" ? null : Math.max(1, parseInt(jobsInput, 10) || 0) || null;
    appSettings = { ...appSettings, jobs: n };
    await setSettings(appSettings.language, n);
  }
  async function chooseDataDir() {
    const dir = await pickFolder();
    if (!dir) return;
    // GNU make can't handle a space in STAPPLER_ROOT, so the data dir must be
    // space-free.
    if (/\s/.test(dir)) {
      showToast(S["datadir-no-space"]);
      return;
    }
    try {
      await setDataDir(dir);
      appSettings = await getSettings();
    } catch (e) {
      error = String(e);
    }
  }
  async function resetDataDir() {
    await setDataDir(null);
    appSettings = await getSettings();
  }

  async function computeUpdateLabels() {
    if (!catalog) return;
    const out: Record<string, string> = {};
    for (const r of catalog.rows) {
      if (r.status.status === "update-available") {
        out[key(r)] = await t("status-update-available", { version: r.status.latest_release });
      }
    }
    updateLabels = out;
  }

  async function refresh() {
    loading = true;
    error = null;
    try {
      catalog = await loadCatalog();
      await computeUpdateLabels();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function setStatus(row: CatalogRow, status: CatalogRow["status"]) {
    if (!catalog) return;
    const r = catalog.rows.find((x) => x.id === row.id && x.kind === row.kind);
    if (r) r.status = status;
  }

  async function installSelected() {
    if (!selectedCount || busy) return;
    busy = true;
    error = null;
    try {
      for (const row of selectedRows) {
        progress[key(row)] = { id: row.id, phase: "downloading", bytes: 0 };
        await install(row.id, row.kind);
        delete progress[key(row)];
        progress = { ...progress };
        selected[key(row)] = false;
        // Optimistic: mark installed locally — no full catalogue reload/flash.
        setStatus(row, { status: "installed" });
      }
    } catch (e) {
      error = String(e);
    } finally {
      progress = {};
      busy = false;
    }
  }

  // window.confirm() is a no-op in the Tauri webview, so use an in-app modal.
  let pending = $state<{ row: CatalogRow; msg: string } | null>(null);

  async function removeRow(row: CatalogRow) {
    if (busy) return;
    pending = { row, msg: await t("confirm-delete", { name: displayName(row) }) };
  }

  async function confirmDelete() {
    const p = pending;
    pending = null;
    if (!p) return;
    busy = true;
    error = null;
    try {
      await uninstall(p.row.id, p.row.kind);
      setStatus(p.row, { status: "not-installed" });
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  function toggle(r: CatalogRow) {
    selected[key(r)] = !selected[key(r)];
  }

  // One-click: engine + native host + native target.
  async function installEverything() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await installForSystem();
      await refresh();
      engine = await engineStatus();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  // Repair = re-download + overwrite the installed component.
  async function repair(row: CatalogRow) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      progress[key(row)] = { id: row.id, phase: "downloading", bytes: 0 };
      await install(row.id, row.kind);
    } catch (e) {
      error = String(e);
    } finally {
      progress = {};
      busy = false;
    }
  }

  // ---- storage / disk usage ----
  let storageOpen = $state(false);
  let storage = $state<Storage | null>(null);
  let storageLoading = $state(false);
  async function openStorage() {
    settingsOpen = false;
    storageOpen = true;
    storageLoading = true;
    try {
      storage = await diskUsage();
    } catch (e) {
      error = String(e);
    } finally {
      storageLoading = false;
    }
  }
  async function pruneEngine(ref: string) {
    try {
      await removeEngine(ref);
      await openStorage();
      engine = await engineStatus();
    } catch (e) {
      error = String(e);
    }
  }
  function fmtBytes(n: number): string {
    if (n >= 1e9) return (n / 1e9).toFixed(1) + " GB";
    if (n >= 1e6) return (n / 1e6).toFixed(0) + " MB";
    if (n >= 1e3) return (n / 1e3).toFixed(0) + " KB";
    return n + " B";
  }

  // ---- diagnostics + doctor ----
  let toast = $state<string | null>(null);
  function showToast(msg: string) {
    toast = msg;
    setTimeout(() => (toast = null), 2200);
  }
  async function copyReport() {
    settingsOpen = false;
    try {
      const report = await diagnosticsReport();
      await navigator.clipboard.writeText(report);
      showToast(S["report-copied"] ?? "Copied");
    } catch (e) {
      error = String(e);
    }
  }
  let doctorModal = $state(false);
  let doctorChecks = $state<DoctorCheck[]>([]);
  let doctorLoading = $state(false);
  async function openDoctor() {
    settingsOpen = false;
    doctorModal = true;
    doctorLoading = true;
    try {
      doctorChecks = await runDoctor();
    } catch (e) {
      error = String(e);
    } finally {
      doctorLoading = false;
    }
  }

  // ---- onboarding hero ----
  let bootHostInstalled = $state(false);
  let bootTargets = $state(0);
  let manualMode = $state(false); // user chose "detailed settings"
  let heroPreview = $state(false); // gear → preview onboarding even when set up
  let heroBooting = $state(false);
  let heroStep = $state<"engine" | "host" | "target" | "">("");
  let heroBytes = $state(0);
  let heroTotal = $state(0); // total size of the current step (0 = unknown)
  let heroPhase = $state("downloading");
  let heroPct = $state<number | null>(null); // null = size unknown (show bytes)
  let heroDone = $state(false);
  let heroError = $state<string | null>(null);
  const sizeOf = (id: string) => catalog?.rows.find((r) => r.id === id)?.size ?? 0;
  // Truly empty system → show the onboarding hero.
  const freshSystem = $derived(!engine && !bootHostInstalled && bootTargets === 0);
  // Don't decide hero-vs-shell until the first boot-state probe resolves —
  // otherwise the hero flashes for a frame on the default (all-empty) state.
  let bootChecked = $state(false);
  const showHero = $derived(bootChecked && ((freshSystem && !manualMode) || heroPreview));

  async function loadBootState() {
    try {
      const tg = await projectTargets();
      bootHostInstalled = tg.hostInstalled;
      bootTargets = tg.targets.length;
    } catch {
      /* offline / not ready */
    }
  }
  async function startBootstrap() {
    if (heroBooting) return;
    heroBooting = true;
    heroDone = false;
    heroError = null;
    heroStep = "engine";
    heroBytes = 0;
    heroTotal = 0;
    heroPhase = "downloading";
    heroPct = null;
    error = null;
    try {
      await installForSystem();
      heroStep = "";
      heroBooting = false;
      heroDone = true; // shows the "create first project" CTA
      engine = await engineStatus();
      await loadBootState();
      await refresh();
    } catch (e) {
      heroError = String(e);
      heroBooting = false;
      heroStep = "";
    }
  }
  function stepDone(k: string): boolean {
    const order = ["engine", "host", "target"];
    return order.indexOf(k) < order.indexOf(heroStep || "engine");
  }

  // Leave the hero and jump straight into the New Project form.
  let createSignal = $state(0);
  function goCreateProject() {
    heroPreview = false;
    manualMode = true;
    heroDone = false;
    tab = "projects";
    createSignal++;
  }

  async function prepareSdk() {
    if (enginePrep !== null) return;
    enginePrep = 0;
    error = null;
    try {
      engine = await prepareEngine();
    } catch (e) {
      error = String(e);
    } finally {
      enginePrep = null;
    }
  }

  onMount(() => {
    // Settings first so the right language is active before strings load.
    loadSettings().then(loadStrings);
    refresh();
    // Resolve engine + boot state before unveiling the UI, so the hero/shell
    // choice is made from real data, not the empty defaults.
    Promise.all([
      engineStatus().then((e) => (engine = e)),
      loadBootState(),
    ]).finally(() => (bootChecked = true));
    const un = onInstallProgress((p) => {
      for (const row of selectedRows) if (row.id === p.id) progress[key(row)] = p;
      progress = { ...progress };
      // Onboarding hero: map by kind (plain target shares the host's id).
      if (heroBooting) {
        heroStep = p.kind === "host" ? "host" : "target";
        heroPhase = p.phase;
        heroBytes = p.bytes;
        const size = sizeOf(p.id);
        heroTotal = size;
        heroPct =
          p.phase === "downloading"
            ? size
              ? Math.min(100, (p.bytes / size) * 100)
              : null
            : 100; // verify/extract/place run after the download completes
      }
    });
    // The engine bundle also downloads automatically before the first toolchain
    // install; reflect that progress and pick up the version when it finishes.
    const unEng = onEngineProgress((p) => {
      if (p.phase === "done") {
        enginePrep = null;
        enginePrepTotal = 0;
        engineStatus().then((e) => (engine = e));
      } else {
        enginePrep = p.bytes;
        enginePrepTotal = p.total;
      }
      if (heroBooting && p.phase !== "done") {
        heroStep = "engine";
        heroPhase = "downloading";
        heroBytes = p.bytes;
        heroTotal = p.total;
        // Determinate bar when the server sent a Content-Length; otherwise fall
        // back to the byte counter so it never looks frozen.
        heroPct = p.total > 0 ? Math.min(100, (p.bytes / p.total) * 100) : null;
      }
    });
    return () => {
      un.then((f) => f());
      unEng.then((f) => f());
    };
  });
</script>

{#if !bootChecked}
  <div class="boot-splash">
    <img class="boot-logo" src={logoUrl} alt="Xenolith" />
    <div class="boot-spinner"></div>
  </div>
{:else if showHero}
  <div class="hero">
    <div class="hero-card">
      <img class="hero-logo" src={logoUrl} alt="Xenolith" />
      <h1 class="hero-title">Xenolith</h1>
      <p class="hero-sub">{S["hero-sub"]}</p>

      {#if heroDone}
        <div class="hero-done">✓ {S["hero-done"]}</div>
        <button class="btn primary hero-btn" onclick={goCreateProject}>
          {S["hero-create"]} <span class="harrow">→</span>
        </button>
      {:else if heroError}
        <p class="hero-err">⚠ {heroError}</p>
        <button class="btn primary hero-btn" onclick={startBootstrap}>↻ {S["hero-retry"]}</button>
        <button class="hero-link" onclick={() => { heroPreview = false; manualMode = true; }}>
          {S["hero-manual"]}
        </button>
      {:else if heroBooting}
        <div class="hero-steps">
          {#each [["engine", S["hero-step-engine"]], ["host", S["hero-step-host"]], ["target", S["hero-step-target"]]] as step (step[0])}
            <div class="hero-step" class:active={heroStep === step[0]} class:done={stepDone(step[0])}>
              <span class="hdot"></span><span class="hlabel">{step[1]}</span>
              {#if heroStep === step[0]}
                <span class="hbytes">
                  {S["phase-" + heroPhase] ?? heroPhase}{#if heroPhase === "downloading"} · {#if heroPct != null}{Math.round(heroPct)}%{#if heroTotal > 0} · {fmtBytes(heroBytes)} / {fmtBytes(heroTotal)}{/if}{:else}{fmtBytes(heroBytes)}{/if}{/if}
                </span>
              {/if}
            </div>
          {/each}
        </div>
        <div class="hero-bar">
          {#if heroPct != null}
            <div class="hero-fill det" style="width:{heroPct}%"></div>
          {:else}
            <div class="hero-fill"></div>
          {/if}
        </div>
      {:else}
        <button class="btn primary hero-btn" onclick={startBootstrap}>⚡ {S["install-all"]}</button>
        <p class="hero-note">{S["install-all-hint"]}</p>
        <button class="hero-link" onclick={() => { heroPreview = false; manualMode = true; }}>
          {S["hero-manual"]}
        </button>
      {/if}
    </div>
  </div>
{:else}
<div class="shell">
  <header>
    <h1>Xenolith Installer</h1>
    <nav class="tabs">
      <button class:active={tab === "packages"} onclick={() => (tab = "packages")}>
        {S["nav-packages"]}
      </button>
      <button class:active={tab === "projects"} onclick={() => (tab = "projects")}>
        {S["nav-projects"]}
      </button>
    </nav>
    <div class="meta">
      {#if catalog && tab === "packages"}
        <span>release {catalog.release}</span>
        {#if catalog.nativeId}<span class="native">host {catalog.nativeId}</span>{/if}
      {/if}
    </div>
    <div class="settings">
      <button
        class="gear"
        aria-label="settings"
        onclick={(e) => {
          e.stopPropagation();
          settingsOpen = !settingsOpen;
        }}
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
      {#if settingsOpen}
        <div class="settings-menu">
          <button class="settings-item" onclick={() => { settingsOpen = false; openWorkingDir(); }}>
            📂 {S["open-working-dir"]}
          </button>
          <button class="settings-item" onclick={openStorage}>
            💾 {S["storage-title"]}
          </button>
          <button class="settings-item" onclick={openSettings}>
            ⚙️ {S["settings-title"]}
          </button>
          <button class="settings-item" onclick={openDoctor}>
            🩺 {S["doctor-title"]}
          </button>
          <button class="settings-item" onclick={copyReport}>
            📋 {S["report-copy"]}
          </button>
          <!-- Onboarding preview — kept for later, hidden from the gear menu for now.
          <button class="settings-item" onclick={() => { settingsOpen = false; heroPreview = true; }}>
            ✨ {S["hero-preview"]}
          </button>
          -->
        </div>
      {/if}
    </div>
  </header>

  {#if tab === "projects"}
    <main><Projects {S} {createSignal} goToPackages={() => (tab = "packages")} /></main>
  {:else}
  <main>
    {#if loading}
      <p class="muted">{S["loading"]}</p>
    {:else if error}
      <p class="error">{error}</p>
      <button class="btn ghost" onclick={refresh}>{S["action-refresh"]}</button>
    {:else if catalog}
      <div class="table">
        <div class="row head">
          <span class="c-check"></span>
          <span class="c-name">{S["col-name"]}</span>
          <span class="c-size">{S["col-size"]}</span>
          <span class="c-status">{S["col-status"]}</span>
        </div>

        <button class="group" onclick={() => (engineCollapsed = !engineCollapsed)}>
          <svg class="chev" class:open={!engineCollapsed} width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
          {S["group-engine"]}
        </button>
        {#if !engineCollapsed}
          <div class="row native">
            <span class="c-check"></span>
            <span class="c-name">
              {#if engine}{engine.reference}<span class="variant"> · {engine.short}</span>{:else}master{/if}
            </span>
            <span class="c-size">—</span>
            <span class="c-status">
              {#if enginePrep !== null}
                <div class="dl">
                  <div class="bar"><div class="fill" class:pulse={enginePrepTotal === 0} style="width:{enginePrepTotal > 0 ? Math.min(100, (enginePrep / enginePrepTotal) * 100) : 100}%"></div></div>
                  <span class="pct">{enginePrepTotal > 0 ? Math.round((enginePrep / enginePrepTotal) * 100) + "%" : fmtBytes(enginePrep)}</span>
                </div>
              {:else if engine}
                <span class="badge installed">{S["status-installed"]}</span>
                <button class="repair" title={S["engine-update"]} onclick={prepareSdk} disabled={busy} aria-label={S["engine-update"]}>
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6" />
                  </svg>
                </button>
              {:else}
                <button class="btn primary sm" onclick={prepareSdk} disabled={busy}>{S["engine-prepare"]}</button>
              {/if}
            </span>
          </div>
        {/if}

        {#each GROUP_ORDER as kind (kind)}
          <button class="group" onclick={() => (collapsed[kind] = !collapsed[kind])}>
            <svg class="chev" class:open={!collapsed[kind]} width="12" height="12" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M6 9l6 6 6-6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
            {groupTitle(kind)}
          </button>
          {#if !collapsed[kind]}
            {#each rowsFor(kind) as row (key(row))}
              {@const installed = row.status.status === "installed"}
              <div class="row" class:native={isNative(row)}>
                <span class="c-check">
                  {#if installed}
                    <button class="trash" title={S["action-delete"]} onclick={() => removeRow(row)} disabled={busy} aria-label={S["action-delete"]}>
                      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m2 0v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6" />
                      </svg>
                    </button>
                  {:else}
                    <label class="cb">
                      <input type="checkbox" checked={!!selected[key(row)]} onchange={() => toggle(row)} disabled={busy} />
                      <span class="box"></span>
                    </label>
                  {/if}
                </span>
                <span class="c-name">
                  {row.triple}{#if row.variant}<span class="variant"> +{row.variant}</span>{/if}
                </span>
                <span class="c-size">{fmtSize(row.size)}</span>
                <span class="c-status">
                  {#if progress[key(row)]}
                    {@const p = progress[key(row)]}
                    <div class="dl">
                      <div class="bar"><div class="fill" class:pulse={p.phase !== "downloading"} style="width:{pct(p, row.size)}%"></div></div>
                      <span class="pct">{pLabel(p, row.size)}</span>
                    </div>
                  {:else}
                    <span class="badge {row.status.status}">{statusText(row)}</span>
                    {#if installed}
                      <button class="repair" title={S["action-repair"]} onclick={() => repair(row)} disabled={busy} aria-label={S["action-repair"]}>
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6" />
                        </svg>
                      </button>
                    {/if}
                  {/if}
                </span>
              </div>
            {/each}
          {/if}
        {/each}
      </div>
    {/if}
  </main>

  <footer class="glass">
    <button class="btn ghost" onclick={refresh} disabled={loading || busy}>{S["action-refresh"]}</button>
    <button class="btn ghost" onclick={installEverything} disabled={busy || loading} title={S["install-all-hint"]}>
      ⚡ {S["install-all"]}
    </button>
    <button class="btn primary" onclick={installSelected} disabled={!selectedCount || busy}>
      {busy ? S["action-installing"] : `${S["action-install"]} (${selectedCount})`}
    </button>
  </footer>
  {/if}

  {#if pending}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="overlay" onclick={() => (pending = null)}>
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <div class="dialog" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
        <p class="dialog-msg">{pending.msg}</p>
        <div class="dialog-actions">
          <button class="btn ghost" onclick={() => (pending = null)}>{S["action-cancel"]}</button>
          <button class="btn danger" onclick={confirmDelete}>{S["action-delete"]}</button>
        </div>
      </div>
    </div>
  {/if}

  {#if storageOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="overlay" onclick={() => (storageOpen = false)}>
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <div class="dialog storage" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
        <div class="storage-head">
          <h3>💾 {S["storage-title"]}</h3>
          {#if storage}<span class="muted">{S["storage-total"]}: {fmtBytes(storage.total)}</span>{/if}
        </div>
        {#if storageLoading}
          <p class="muted">{S["loading"]}</p>
        {:else if storage}
          {#each [{ k: "engines", label: S["storage-engines"], prune: true }, { k: "hosts", label: S["storage-hosts"], prune: false }, { k: "targets", label: S["storage-targets"], prune: false }] as grp (grp.k)}
            {@const items = storage[grp.k as "engines" | "hosts" | "targets"]}
            {#if items.length}
              <div class="storage-group">{grp.label}</div>
              {#each items as it (it.id)}
                <div class="storage-row">
                  <span class="sid">{it.id}</span>
                  <span class="sbytes">{fmtBytes(it.bytes)}</span>
                  {#if grp.prune}
                    <button class="btn ghost danger sm" onclick={() => pruneEngine(it.id)}>{S["action-delete"]}</button>
                  {:else}
                    <span class="sgap"></span>
                  {/if}
                </div>
              {/each}
            {/if}
          {/each}
        {/if}
        <div class="dialog-actions">
          <button class="btn ghost" onclick={() => (storageOpen = false)}>{S["action-cancel"]}</button>
        </div>
      </div>
    </div>
  {/if}

  {#if settingsModal && appSettings}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="overlay" onclick={() => (settingsModal = false)}>
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <div class="dialog storage" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
        <div class="storage-head"><h3>⚙️ {S["settings-title"]}</h3></div>

        <div class="set-row">
          <span class="set-label">{S["settings-language"]}</span>
          <div class="seg">
            <button class="seg-btn" class:on={appSettings.language == null} onclick={() => chooseLanguage(null)}>{S["settings-auto"]}</button>
            <button class="seg-btn" class:on={appSettings.language === "en"} onclick={() => chooseLanguage("en")}>English</button>
            <button class="seg-btn" class:on={appSettings.language === "ru"} onclick={() => chooseLanguage("ru")}>Русский</button>
            <button class="seg-btn" class:on={appSettings.language === "zh"} onclick={() => chooseLanguage("zh")}>中文</button>
          </div>
        </div>

        <div class="set-row">
          <span class="set-label">{S["settings-jobs"]}</span>
          <div class="set-control">
            <input
              class="jobs-input"
              type="number"
              min="1"
              placeholder={`${S["settings-auto"]} (${appSettings.autoJobs})`}
              bind:value={jobsInput}
              onchange={applyJobs}
            />
          </div>
        </div>

        <div class="set-row col">
          <span class="set-label">{S["settings-datadir"]}</span>
          <code class="datadir">{appSettings.dataDir}</code>
          <div class="set-control">
            <button class="btn ghost sm" onclick={chooseDataDir}>{S["project-choose"]}</button>
            {#if appSettings.dataDirOverride}
              <button class="btn ghost sm" onclick={resetDataDir}>{S["settings-reset"]}</button>
            {/if}
          </div>
          <span class="muted set-note">{S["settings-restart"]}</span>
        </div>

        <div class="dialog-actions">
          <button class="btn primary" onclick={() => (settingsModal = false)}>{S["action-cancel"]}</button>
        </div>
      </div>
    </div>
  {/if}

  {#if doctorModal}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="overlay" onclick={() => (doctorModal = false)}>
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <div class="dialog storage" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
        <div class="storage-head"><h3>🩺 {S["doctor-title"]}</h3></div>
        {#if doctorLoading}
          <p class="muted">{S["loading"]}</p>
        {:else}
          {#each doctorChecks as c (c.name)}
            <div class="doc-row">
              <span class="doc-mark {c.ok ? 'ok' : 'bad'}">{c.ok ? "✓" : "✗"}</span>
              <span class="doc-name">{c.name}</span>
              <span class="doc-detail">{c.detail}</span>
            </div>
          {/each}
        {/if}
        <div class="dialog-actions">
          <button class="btn ghost" onclick={openDoctor}>{S["action-refresh"]}</button>
          <button class="btn primary" onclick={() => (doctorModal = false)}>{S["action-cancel"]}</button>
        </div>
      </div>
    </div>
  {/if}

  {#if toast}<div class="toast">{toast}</div>{/if}
</div>
{/if}

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape") {
      pending = null;
      storageOpen = false;
      settingsModal = false;
      doctorModal = false;
    }
  }}
  onclick={() => (settingsOpen = false)}
/>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--xeno-bg-page);
  }
  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding: 18px 24px 12px;
  }
  h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }
  .meta {
    display: flex;
    gap: 16px;
    color: var(--xeno-text-secondary);
    font-size: 12px;
  }
  .tabs {
    display: flex;
    gap: 4px;
    margin-left: 20px;
    flex: 1;
  }
  .tabs button {
    background: transparent;
    border: none;
    color: var(--xeno-text-secondary);
    font-size: 13px;
    font-weight: 600;
    padding: 6px 12px;
    border-radius: var(--xeno-radius-control);
  }
  .tabs button.active {
    color: var(--xeno-accent);
    background: rgba(252, 180, 0, 0.08);
  }
  .settings {
    position: relative;
    margin-left: 12px;
  }
  .gear {
    display: inline-flex;
    background: transparent;
    border: none;
    color: var(--xeno-text-secondary);
    padding: 4px;
    border-radius: 6px;
  }
  .gear:hover {
    color: var(--xeno-accent);
  }
  .settings-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 30;
    min-width: 220px;
    background: var(--xeno-surface-2);
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    padding: 4px;
  }
  .settings-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--xeno-text);
    font-size: 13px;
    padding: 8px 10px;
    border-radius: 6px;
  }
  .settings-item:hover {
    background: rgba(252, 180, 0, 0.1);
  }
  .repair {
    display: inline-flex;
    vertical-align: middle;
    margin-left: 8px;
    background: transparent;
    border: none;
    color: var(--xeno-text-secondary);
    padding: 2px;
    border-radius: 4px;
  }
  .repair:hover:not(:disabled) {
    color: var(--xeno-accent);
  }
  .repair:disabled {
    opacity: 0.4;
  }
  .storage {
    width: min(560px, 92vw);
    text-align: left;
  }
  .storage-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }
  .storage-head h3 {
    margin: 0;
    font-size: 15px;
  }
  .storage-group {
    margin: 12px 0 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--xeno-text-secondary);
  }
  .storage-row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    align-items: center;
    gap: 12px;
    padding: 5px 0;
    border-bottom: 1px solid var(--xeno-border-muted);
  }
  .sid {
    font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sbytes {
    color: var(--xeno-text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .sgap {
    width: 0;
  }
  .btn.sm {
    padding: 3px 8px;
    font-size: 12px;
  }
  .set-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    border-bottom: 1px solid var(--xeno-border-muted);
  }
  .set-row.col {
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }
  .set-label {
    font-size: 13px;
    color: var(--xeno-text-secondary);
  }
  .set-control {
    display: flex;
    gap: 8px;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    overflow: hidden;
  }
  .seg-btn {
    background: transparent;
    border: none;
    color: var(--xeno-text-secondary);
    font-size: 13px;
    padding: 6px 12px;
  }
  .seg-btn:not(:last-child) {
    border-right: 1px solid var(--xeno-border-muted);
  }
  .seg-btn.on {
    background: var(--xeno-accent);
    color: var(--xeno-on-accent);
    font-weight: 600;
  }
  .jobs-input {
    width: 130px;
    background: var(--xeno-bg);
    border: 1px solid var(--xeno-border);
    border-radius: var(--xeno-radius-control);
    color: var(--xeno-text);
    padding: 6px 8px;
    font-size: 13px;
  }
  .datadir {
    font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 12px;
    background: var(--xeno-bg);
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    padding: 6px 8px;
    overflow-wrap: anywhere;
  }
  .set-note {
    font-size: 11px;
  }
  /* ---- onboarding hero ---- */
  .hero {
    height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background:
      radial-gradient(900px 500px at 50% -10%, rgba(252, 180, 0, 0.12), transparent 60%),
      var(--xeno-bg-page);
  }
  .boot-splash {
    height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 22px;
    background: var(--xeno-bg-page);
  }
  .boot-logo {
    width: 84px;
    height: 84px;
    object-fit: contain;
    filter: drop-shadow(0 8px 28px rgba(252, 180, 0, 0.25));
    animation: pulse 1.6s ease-in-out infinite;
  }
  .boot-spinner {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 3px solid var(--xeno-border);
    border-top-color: var(--xeno-accent, #fcb400);
    animation: spin 0.7s linear infinite;
  }
  .hero-card {
    width: min(440px, 92vw);
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 10px;
  }
  .hero-logo {
    width: 112px;
    height: 112px;
    object-fit: contain;
    filter: drop-shadow(0 8px 28px rgba(252, 180, 0, 0.25));
  }
  .hero-title {
    margin: 4px 0 0;
    font-size: 34px;
    font-weight: 700;
    letter-spacing: 0.01em;
  }
  .hero-sub {
    margin: 0 0 12px;
    color: var(--xeno-text-secondary);
    font-size: 14px;
    max-width: 340px;
  }
  .hero-btn {
    font-size: 16px;
    font-weight: 600;
    padding: 12px 28px;
    border-radius: 10px;
  }
  .hero-note {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--xeno-text-secondary);
  }
  .hero-link {
    margin-top: 10px;
    background: transparent;
    border: none;
    color: var(--xeno-text-secondary);
    text-decoration: underline;
    font-size: 13px;
  }
  .hero-link:hover {
    color: var(--xeno-accent);
  }
  .hero-steps {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 8px 0 4px;
  }
  .hero-step {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--xeno-text-secondary);
    font-size: 14px;
  }
  .hero-step .hdot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid var(--xeno-border-muted);
    flex: 0 0 auto;
  }
  .hero-step.active {
    color: var(--xeno-text);
  }
  .hero-step.active .hdot {
    border-color: var(--xeno-accent);
    background: var(--xeno-accent);
    animation: pulse 1s ease-in-out infinite;
  }
  .hero-step.done .hdot {
    border-color: #6bdc8f;
    background: #6bdc8f;
  }
  .hbytes {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
    font-size: 12px;
    color: var(--xeno-text-secondary);
  }
  .hero-bar {
    width: 100%;
    height: 6px;
    border-radius: 3px;
    background: var(--xeno-surface-3);
    overflow: hidden;
  }
  .hero-fill {
    width: 40%;
    height: 100%;
    border-radius: 3px;
    background: var(--xeno-accent);
    animation: indet 1.1s ease-in-out infinite;
  }
  .hero-fill.det {
    animation: none;
    transition: width 0.2s ease;
  }
  .hero-done {
    font-size: 18px;
    font-weight: 600;
    color: #6bdc8f;
    padding: 12px 12px 6px;
  }
  .harrow {
    display: inline-block;
    transition: transform 0.15s ease;
  }
  .hero-btn:hover .harrow {
    transform: translateX(3px);
  }
  .hero-err {
    color: #ff6b6b;
    font-size: 13px;
    margin: 0 0 12px;
    max-width: 360px;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  @keyframes indet {
    0% {
      transform: translateX(-110%);
    }
    100% {
      transform: translateX(360%);
    }
  }
  /* ---- doctor ---- */
  .doc-row {
    display: grid;
    grid-template-columns: 18px 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 7px 0;
    border-bottom: 1px solid var(--xeno-border-muted);
    font-size: 13px;
  }
  .doc-mark.ok {
    color: #6bdc8f;
  }
  .doc-mark.bad {
    color: #ff6b6b;
  }
  .doc-detail {
    color: var(--xeno-text-secondary);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 220px;
  }
  /* ---- toast ---- */
  .toast {
    position: fixed;
    bottom: 28px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--xeno-surface-3);
    border: 1px solid var(--xeno-border-muted);
    color: var(--xeno-text);
    padding: 10px 18px;
    border-radius: 8px;
    font-size: 13px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    z-index: 100;
  }
  .native {
    color: var(--xeno-accent);
  }
  main {
    flex: 1;
    overflow-y: auto;
    padding: 6px 24px 96px;
  }
  .table {
    background: var(--xeno-bg);
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-card);
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: 44px 1fr 90px 200px;
    align-items: center;
    gap: 12px;
    padding: 9px 16px;
    border-bottom: 1px solid var(--xeno-border-muted);
  }
  .row.head {
    color: var(--xeno-text-secondary);
    font-size: 12px;
    background: var(--xeno-surface-2);
  }
  .row.native {
    background: rgba(252, 180, 0, 0.05);
  }
  .group {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 9px 16px;
    background: transparent;
    border: none;
    border-bottom: 1px solid var(--xeno-border-muted);
    color: var(--xeno-text);
    font-size: 13px;
    font-weight: 600;
  }
  .chev {
    flex: 0 0 auto;
    transition: transform 0.15s;
    transform: rotate(-90deg);
    color: var(--xeno-text-secondary);
  }
  .chev.open {
    transform: rotate(0deg);
  }
  .c-check {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .c-name {
    font-variant-numeric: tabular-nums;
  }
  .variant {
    color: var(--xeno-accent);
  }
  .c-size {
    color: var(--xeno-text-secondary);
    font-size: 12px;
  }
  .badge {
    font-size: 12px;
    white-space: nowrap;
  }
  .badge.installed,
  .badge.update-available {
    color: var(--xeno-accent);
  }
  .badge.not-installed {
    color: var(--xeno-text-secondary);
  }
  .dl {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .bar {
    flex: 1;
    height: 6px;
    border-radius: 3px;
    background: var(--xeno-surface-3);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--xeno-accent);
    border-radius: 3px;
    transition: width 0.15s linear;
  }
  .fill.pulse {
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 0.5;
    }
    50% {
      opacity: 1;
    }
  }
  .pct {
    color: var(--xeno-accent);
    font-size: 12px;
    text-transform: capitalize;
    min-width: 64px;
    text-align: right;
  }
  .cb {
    display: inline-flex;
    cursor: pointer;
  }
  .cb input {
    position: absolute;
    opacity: 0;
    width: 0;
    height: 0;
  }
  .cb .box {
    width: 18px;
    height: 18px;
    border-radius: 4px;
    border: 1px solid var(--xeno-border);
    display: inline-block;
    position: relative;
  }
  .cb input:checked + .box {
    background: var(--xeno-accent);
    border-color: var(--xeno-accent);
  }
  .cb input:checked + .box::after {
    content: "";
    position: absolute;
    left: 5px;
    top: 1px;
    width: 5px;
    height: 10px;
    border: solid var(--xeno-on-accent);
    border-width: 0 2px 2px 0;
    transform: rotate(45deg);
  }
  .cb input:disabled + .box {
    opacity: 0.5;
  }
  .trash {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border: none;
    background: transparent;
    color: var(--xeno-text-secondary);
    border-radius: 6px;
  }
  .trash:not(:disabled):hover {
    color: #ff6b6b;
    background: rgba(255, 107, 107, 0.1);
  }
  .btn {
    border: 1px solid var(--xeno-border);
    border-radius: var(--xeno-radius-control);
    padding: 6px 16px;
    background: transparent;
    color: var(--xeno-text);
  }
  .btn.primary {
    background: var(--xeno-accent);
    color: var(--xeno-on-accent);
    border-color: var(--xeno-accent);
    font-weight: 600;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.ghost:not(:disabled):hover {
    border-color: var(--xeno-accent);
  }
  footer.glass {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    padding: 14px 24px;
    background: rgba(26, 26, 26, 0.6);
    backdrop-filter: blur(6px);
    border-top: 1px solid var(--xeno-border-muted);
  }
  .muted {
    color: var(--xeno-text-secondary);
  }
  .error {
    color: #ff6b6b;
  }
  .btn.sm {
    padding: 4px 12px;
    font-size: 12px;
    margin-left: auto;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }
  .dialog {
    width: 380px;
    max-width: calc(100vw - 48px);
    background: var(--xeno-surface-2);
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-card);
    padding: 20px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
  }
  .dialog-msg {
    margin: 0 0 18px;
    line-height: 1.5;
  }
  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 10px;
  }
  .btn.danger {
    background: #ff6b6b;
    color: #1a1a1a;
    border-color: #ff6b6b;
    font-weight: 600;
  }
</style>
