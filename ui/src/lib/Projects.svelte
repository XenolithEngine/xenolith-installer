<script lang="ts">
  import { onMount } from "svelte";
  import {
    projectEngines,
    projectTargets,
    listProjects,
    createProject,
    removeProject,
    buildProject,
    onBuildLine,
    pickFolder,
    availableEditors,
    openInEditor,
    openTerminal,
    cleanProject,
    projectSize,
    cancelBuild,
    type Project,
    type Editor,
  } from "./api";

  let {
    S,
    goToPackages,
    createSignal = 0,
  }: { S: Record<string, string>; goToPackages: () => void; createSignal?: number } = $props();

  let projects = $state<Project[]>([]);
  let engines = $state<string[]>([]);
  let targets = $state<string[]>([]);
  let editors = $state<Editor[]>([]);
  let openMenu = $state<string | null>(null);
  let buildMenu = $state<string | null>(null);
  let host = $state("");
  let consoleEl = $state<HTMLElement | null>(null);
  let view = $state<"list" | "new">("list");
  let hostInstalled = $state(false);
  let name = $state("");
  let location = $state<string | null>(null);
  let engine = $state("");
  let newTarget = $state("");
  let creating = $state(false);
  let error = $state<string | null>(null);

  // Per-project selected build target (defaults to the host).
  let selTarget = $state<Record<string, string>>({});
  let buildingPath = $state<string | null>(null);
  let log = $state<string[]>([]);

  // Name must be non-empty and path/make-safe (no spaces).
  const nameValid = $derived(/^[A-Za-z0-9_-]+$/.test(name));
  const projectPath = $derived(location && name ? `${location}/${name}` : "");
  // Projects can only be created with an engine, the host toolchain, and a target.
  const ready = $derived(engines.length > 0 && hostInstalled && targets.length > 0);
  const canCreate = $derived(nameValid && !!location && !!engine && !!newTarget && ready && !creating);
  const targetOf = (p: Project) => selTarget[p.path] ?? (p.target || host);
  // A same-arch target (e.g. <host>+sprt) is native and runnable; only a
  // different-arch triple is a true cross-compile that can't run here.
  const runnable = (p: Project) => targetOf(p).split("+")[0] === host;

  async function reload() {
    const [pj, en, tg, ed] = await Promise.all([
      listProjects(),
      projectEngines(),
      projectTargets(),
      availableEditors(),
    ]);
    projects = pj;
    engines = en;
    targets = tg.targets;
    host = tg.host;
    hostInstalled = tg.hostInstalled;
    editors = ed;
    if (!engine && engines.length) engine = engines[0];
    // Default target = the host if it's installed, else the first available.
    if (!newTarget || !targets.includes(newTarget)) {
      newTarget = targets.includes(host) ? host : (targets[0] ?? "");
    }
    loadSizes();
  }

  // Per-project on-disk size (mostly stappler-build); filled in after the list.
  let sizes = $state<Record<string, number>>({});
  async function loadSizes() {
    for (const p of projects) {
      projectSize(p.path)
        .then((b) => (sizes = { ...sizes, [p.path]: b }))
        .catch(() => {});
    }
  }
  function fmtBytes(n: number): string {
    if (n >= 1e9) return (n / 1e9).toFixed(1) + " GB";
    if (n >= 1e6) return (n / 1e6).toFixed(0) + " MB";
    if (n >= 1e3) return (n / 1e3).toFixed(0) + " KB";
    return n + " B";
  }

  async function openIn(p: Project, editor: string) {
    openMenu = null;
    try {
      await openInEditor(p.path, editor);
    } catch (e) {
      error = String(e);
    }
  }

  // Auto-scroll the build console to the newest line.
  $effect(() => {
    void log.length;
    if (consoleEl) consoleEl.scrollTop = consoleEl.scrollHeight;
  });

  async function choose() {
    const picked = await pickFolder();
    if (picked) location = picked;
  }

  async function create() {
    if (!canCreate || !location) return;
    creating = true;
    error = null;
    try {
      await createProject(location, name, engine, newTarget);
      name = "";
      view = "list";
      await reload();
    } catch (e) {
      error = String(e);
    } finally {
      creating = false;
    }
  }

  async function build(p: Project, run: boolean) {
    if (buildingPath) return;
    buildingPath = p.path;
    log = [];
    error = null;
    try {
      await buildProject(p.path, targetOf(p), run);
    } catch (e) {
      error = String(e);
    } finally {
      buildingPath = null;
      projectSize(p.path).then((b) => (sizes = { ...sizes, [p.path]: b }));
    }
  }

  async function clean(p: Project) {
    if (buildingPath) return;
    try {
      await cleanProject(p.path);
      log = [...log, `— cleaned ${p.name} —`];
      sizes = { ...sizes, [p.path]: await projectSize(p.path) };
    } catch (e) {
      error = String(e);
    }
  }

  async function rebuild(p: Project) {
    if (buildingPath) return;
    try {
      await cleanProject(p.path);
    } catch (e) {
      error = String(e);
      return;
    }
    await build(p, false);
  }

  async function terminal(p: Project) {
    try {
      await openTerminal(p.path);
    } catch (e) {
      error = String(e);
    }
  }

  async function cancel() {
    try {
      await cancelBuild();
      log = [...log, "— cancelled —"];
    } catch (e) {
      error = String(e);
    }
  }

  // Colour build-console lines: errors red, warnings amber, success green.
  function lineClass(l: string): string {
    if (/(^|[\s[])(error:|error |fatal|\*\*\*)/i.test(l) || l.includes("✗")) return "err";
    if (/(^|\s)warning:/i.test(l)) return "warn";
    if (l.startsWith("✓")) return "ok";
    return "";
  }

  async function remove(p: Project) {
    await removeProject(p.path);
    await reload();
  }

  onMount(() => {
    reload();
    const un = onBuildLine((line) => {
      log = [...log, line];
    });
    return () => un.then((f) => f());
  });

  // The hero's "Create first project" bumps createSignal → open the New form.
  $effect(() => {
    if (createSignal > 0) view = "new";
  });
</script>

<div class="projects">
  {#if view === "new"}
    <section class="new glass">
      <div class="new-head">
        <button class="btn ghost" onclick={() => (view = "list")}>← {S["action-cancel"]}</button>
        <h2>{S["project-new"]}</h2>
      </div>
      {#if !ready}
        <p class="muted">{S["create-requirements"]}</p>
      {:else}
        <div class="form">
          <label>
            <span>{S["project-name"]}</span>
            <input
              type="text"
              bind:value={name}
              placeholder="my-app"
              class:bad={!!name && !nameValid}
            />
            {#if !!name && !nameValid}
              <span class="hint err">{S["project-name-rule"]}</span>
            {:else}
              <span class="hint">{S["project-name-rule"]}</span>
            {/if}
          </label>
          <label>
            <span>{S["project-location"]}</span>
            <div class="path-row">
              <input type="text" readonly value={location ?? ""} placeholder="…" />
              <button class="btn ghost" onclick={choose}>{S["project-choose"]}</button>
            </div>
          </label>
          <label>
            <span>{S["project-engine"]}</span>
            <select bind:value={engine}>
              {#each engines as e (e)}<option value={e}>{e}</option>{/each}
            </select>
          </label>
          <label>
            <span>{S["project-target"]}</span>
            <select bind:value={newTarget}>
              {#each targets as t (t)}
                <option value={t}>{t}{t === host ? " (host)" : ""}</option>
              {/each}
            </select>
          </label>
          {#if projectPath}
            <p class="preview">→ {projectPath}</p>
          {/if}
          {#if error}<p class="error">{error}</p>{/if}
          <button class="btn primary" onclick={create} disabled={!canCreate}>
            {S["project-create"]}
          </button>
        </div>
      {/if}
    </section>
  {:else}
    <div class="list-head">
      <h2>{S["nav-projects"]}</h2>
      <button
        class="btn primary"
        onclick={() => (view = "new")}
        disabled={!ready}
        title={!ready ? S["create-requirements"] : ""}
      >
        + {S["project-new"]}
      </button>
    </div>

    {#if !ready}
      <div class="alert">
        <span>⚠️ {S["create-requirements"]}</span>
        <button class="btn primary sm" onclick={goToPackages}>{S["go-packages"]}</button>
      </div>
    {/if}

    {#if error}<p class="error">{error}</p>{/if}

    <section class="list">
    {#if projects.length === 0}
      <p class="muted">{S["projects-empty"]}</p>
    {:else}
      {#each projects as p (p.path)}
        <div class="proj">
          <div class="info">
            <span class="pname"
              >{p.name}{#if sizes[p.path] != null}<span class="psize"> · {fmtBytes(sizes[p.path])}</span>{/if}</span
            >
            <span class="ppath">{p.path}</span>
            <span class="peng">{S["engine-label"]} {p.engine}</span>
          </div>
          <div class="acts">
            <select
              class="tsel"
              value={targetOf(p)}
              onchange={(e) => (selTarget[p.path] = e.currentTarget.value)}
              disabled={!!buildingPath}
              title={S["project-target"]}
            >
              {#each targets as t (t)}<option value={t}>{t}</option>{/each}
            </select>
            <button
              class="btn primary"
              onclick={() => build(p, true)}
              disabled={!!buildingPath || !runnable(p)}
              title={!runnable(p) ? S["run-host-only"] : ""}
            >
              {S["project-run"]}
            </button>
            <div class="split">
              <button class="btn ghost split-main" onclick={() => build(p, false)} disabled={!!buildingPath}>
                {buildingPath === p.path ? S["build-building"] : S["project-build"]}
              </button>
              <button
                class="btn ghost split-chev"
                onclick={(e) => {
                  e.stopPropagation();
                  buildMenu = buildMenu === p.path ? null : p.path;
                }}
                disabled={!!buildingPath}
                aria-label={S["project-rebuild"]}
              >
                <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="m6 9 6 6 6-6" />
                </svg>
              </button>
              {#if buildMenu === p.path}
                <div class="menu build-menu">
                  <button class="menu-item" onclick={() => { buildMenu = null; rebuild(p); }}>
                    <svg class="eicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                      <path d="M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6" />
                    </svg>
                    {S["project-rebuild"]}
                  </button>
                  <button class="menu-item" onclick={() => { buildMenu = null; clean(p); }}>
                    <svg class="eicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                      <path d="m7 21-4.3-4.3a1 1 0 0 1 0-1.4l9.6-9.6a1 1 0 0 1 1.4 0l5.6 5.6a1 1 0 0 1 0 1.4L13 21" />
                      <path d="M22 21H7" /><path d="m5 11 9 9" />
                    </svg>
                    {S["project-clean"]}
                  </button>
                </div>
              {/if}
            </div>
            {#if buildingPath === p.path}
              <button class="btn ghost danger" onclick={cancel}>{S["action-cancel"]}</button>
            {/if}
            {#if editors.length}
              <div class="open-wrap">
                <button
                  class="btn ghost"
                  onclick={(e) => {
                    e.stopPropagation();
                    openMenu = openMenu === p.path ? null : p.path;
                  }}
                >
                  {S["project-open"]}
                  <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="m6 9 6 6 6-6" />
                  </svg>
                </button>
                {#if openMenu === p.path}
                  <div class="menu">
                    <button class="menu-item" onclick={() => { openMenu = null; terminal(p); }}>
                      <svg class="eicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                        <path d="m4 17 6-6-6-6" /><path d="M12 19h8" />
                      </svg>
                      {S["project-terminal"]}
                    </button>
                    {#each editors as ed (ed.id)}
                      <button class="menu-item" onclick={() => openIn(p, ed.id)}>
                        {#if ed.id === "files"}
                          <svg class="eicon" viewBox="0 0 24 24" aria-hidden="true">
                            <path
                              fill="#7fb2ff"
                              d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"
                            />
                          </svg>
                        {:else if ed.id === "vscode"}
                          <svg class="eicon" viewBox="0 0 24 24" aria-hidden="true">
                            <path
                              fill="#0098FF"
                              d="M23.15 2.587 18.21.21a1.494 1.494 0 0 0-1.705.29l-9.46 8.63-4.12-3.128a.999.999 0 0 0-1.276.057L.327 7.261A1 1 0 0 0 .326 8.74L3.899 12 .326 15.26a1 1 0 0 0 .001 1.479L1.65 17.94a.999.999 0 0 0 1.276.057l4.12-3.128 9.46 8.63a1.492 1.492 0 0 0 1.704.29l4.942-2.377A1.5 1.5 0 0 0 24 18.082V5.918a1.5 1.5 0 0 0-.85-1.331zm-5.146 14.861L10.826 12l7.178-5.448z"
                            />
                          </svg>
                        {:else if ed.id === "cursor"}
                          <svg class="eicon" viewBox="0 0 24 24" aria-hidden="true">
                            <path fill="#e6e6e6" d="M12 2 3.5 6.75 12 11.5l8.5-4.75z" />
                            <path fill="#9c9c9c" d="M3.5 6.75v9.5L12 21v-9.5z" />
                            <path fill="#c2c2c2" d="M20.5 6.75v9.5L12 21v-9.5z" />
                          </svg>
                        {:else}
                          <svg class="eicon" viewBox="0 0 24 24" aria-hidden="true">
                            <path
                              fill="#D97757"
                              d="M4.709 15.955l4.72-2.647.079-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.365-.462-.158-1.008.656-.722.881.06.225.061 2.213 1.688 1.328.975 1.95 1.435.285.239.115-.082.014-.058-.128-.214-1.066-1.926-1.14-1.962-.508-.814-.134-.488a2.345 2.345 0 0 1-.082-.578l.748-1.017.414-.134.998.134.42.364.618 1.42 1.005 2.233 1.557 3.037.456.9.242.832.091.255h.158v-.146l.128-1.72.237-2.113.23-2.72.078-.765.376-.91.749-.492.584.28.48.685-.066.444-.287 1.852-.558 2.903-.365 1.943h.213l.243-.243.984-1.309 1.652-2.063.728-.82.85-.904.547-.43h1.033l.76 1.128-.34 1.165-1.064 1.349-.881 1.14-1.263 1.7-.79 1.36.073.11.188-.02 2.857-.605 1.542-.28 1.842-.315.833.388.09.394-.327.808-1.969.485-2.31.463-3.438.812-.043.03.05.062 1.55.146.662.036h1.622l3.02.225.79.522.474.638-.08.485-1.215.62-1.64-.389-3.828-.91-1.313-.327h-.182v.108l1.094 1.068 2.006 1.81 2.509 2.33.128.578-.322.455-.34-.049-2.205-1.657-.851-.748-1.926-1.62h-.128v.17l.443.649 2.345 3.52.122 1.082-.17.353-.607.213-.668-.122-1.374-1.926-1.415-2.167-1.143-1.943-.14.08-.673 7.254-.316.37-.728.28-.607-.461-.322-.747.322-1.476.388-1.924.316-1.53.285-1.9.17-.629-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.929-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.06-.745.231-.243 1.908-1.312z"
                            />
                          </svg>
                        {/if}
                        {ed.name}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {/if}
            <button
              class="iconbtn danger"
              onclick={() => remove(p)}
              title={S["project-remove"]}
              aria-label={S["project-remove"]}
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2m2 0v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V6" />
              </svg>
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </section>

    {#if log.length}
      <div class="console" bind:this={consoleEl}>
        {#each log as line, i (i)}<div class="ln {lineClass(line)}">{line || " "}</div>{/each}
      </div>
    {/if}
  {/if}
</div>

<svelte:window onclick={() => { openMenu = null; buildMenu = null; }} />

<style>
  .projects {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .new {
    padding: 16px;
    border-radius: var(--xeno-radius-card);
    border: 1px solid var(--xeno-border-muted);
  }
  h2 {
    margin: 0 0 12px;
    font-size: 15px;
    font-weight: 600;
  }
  .new-head {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 12px;
  }
  .new-head h2 {
    margin: 0;
  }
  .alert {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border: 1px solid rgba(252, 180, 0, 0.4);
    background: rgba(252, 180, 0, 0.08);
    border-radius: var(--xeno-radius-card);
    font-size: 13px;
  }
  .alert span {
    flex: 1;
  }
  .btn.sm {
    padding: 5px 12px;
    font-size: 12px;
    white-space: nowrap;
  }
  .list-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .list-head h2 {
    margin: 0;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 560px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--xeno-text-secondary);
  }
  input,
  select {
    background: var(--xeno-bg);
    border: 1px solid var(--xeno-border);
    border-radius: var(--xeno-radius-control);
    color: var(--xeno-text);
    padding: 7px 10px;
    font-size: 13px;
  }
  .path-row {
    display: flex;
    gap: 8px;
  }
  .path-row input {
    flex: 1;
  }
  input.bad {
    border-color: #ff6b6b;
  }
  .hint {
    font-size: 11px;
    color: var(--xeno-text-secondary);
  }
  .hint.err {
    color: #ff6b6b;
  }
  .preview {
    margin: 0;
    font-size: 12px;
    color: var(--xeno-accent);
    word-break: break-all;
  }
  .proj {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 14px;
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-card);
    background: var(--xeno-bg);
    margin-bottom: 8px;
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .pname {
    font-weight: 600;
  }
  .psize {
    font-weight: 400;
    color: var(--xeno-text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .iconbtn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    background: transparent;
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    color: var(--xeno-text-secondary);
    flex: 0 0 auto;
  }
  .iconbtn:hover:not(:disabled) {
    color: var(--xeno-text);
    border-color: var(--xeno-border);
  }
  .iconbtn.danger:hover:not(:disabled) {
    color: #ff6b6b;
    border-color: #ff6b6b;
  }
  .iconbtn:disabled {
    opacity: 0.4;
  }
  .ppath {
    font-size: 12px;
    color: var(--xeno-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .peng {
    font-size: 11px;
    color: var(--xeno-accent);
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1 1 auto;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .tsel {
    order: -2; /* target select stays first, before "Open in" */
    background: var(--xeno-bg);
    border: 1px solid var(--xeno-border);
    border-radius: var(--xeno-radius-control);
    color: var(--xeno-text);
    padding: 5px 8px;
    font-size: 12px;
    max-width: 200px;
  }
  .open-wrap {
    position: relative;
    order: -1; /* "Open in" first among the buttons, right after the target select */
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 20;
    min-width: 170px;
    background: var(--xeno-surface-2);
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
    padding: 4px;
  }
  .menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--xeno-text);
    font-size: 13px;
    padding: 7px 8px;
    border-radius: 6px;
  }
  .menu-item:hover {
    background: rgba(252, 180, 0, 0.1);
  }
  .split {
    position: relative;
    display: inline-flex;
  }
  .btn.split-main {
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
  }
  .btn.split-chev {
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    margin-left: -1px;
    padding-left: 8px;
    padding-right: 8px;
    display: inline-flex;
    align-items: center;
  }
  .chev {
    width: 14px;
    height: 14px;
    vertical-align: middle;
  }
  .eicon {
    width: 18px;
    height: 18px;
    flex: 0 0 auto;
  }
  .btn {
    border: 1px solid var(--xeno-border);
    border-radius: var(--xeno-radius-control);
    padding: 6px 14px;
    background: transparent;
    color: var(--xeno-text);
    font-size: 13px;
  }
  .btn.primary {
    background: var(--xeno-accent);
    color: var(--xeno-on-accent);
    border-color: var(--xeno-accent);
    font-weight: 600;
  }
  .btn.danger:hover {
    color: #ff6b6b;
    border-color: #ff6b6b;
  }
  .btn:disabled {
    opacity: 0.45;
  }
  .console {
    background: #111;
    border: 1px solid var(--xeno-border-muted);
    border-radius: var(--xeno-radius-control);
    padding: 12px;
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
    font-size: 12px;
    line-height: 1.45;
    max-height: 320px;
    overflow: auto;
    margin: 0;
  }
  .ln {
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--xeno-text-secondary);
  }
  .ln.err {
    color: #ff6b6b;
  }
  .ln.warn {
    color: #ffcc66;
  }
  .ln.ok {
    color: #6bdc8f;
    font-weight: 600;
  }
  .muted {
    color: var(--xeno-text-secondary);
  }
  .error {
    color: #ff6b6b;
  }
</style>
