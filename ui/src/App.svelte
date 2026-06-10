<script lang="ts">
  import { onMount } from "svelte";
  import {
    loadCatalog,
    install,
    uninstall,
    onInstallProgress,
    t,
    type Catalog,
    type CatalogRow,
    type InstallProgress,
    type Kind,
  } from "./lib/api";

  // English defaults so the UI never flashes raw keys; loadStrings() localises.
  const DEFAULTS: Record<string, string> = {
    "group-hosts": "Development Tools",
    "group-targets": "Runtime Platforms",
    "status-installed": "Installed",
    "status-not-installed": "Not Installed",
    "action-install": "Install",
    "action-refresh": "Refresh",
    "action-installing": "Installing…",
    "action-delete": "Delete",
    "action-cancel": "Cancel",
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
  let progress = $state<Record<string, InstallProgress>>({});
  let busy = $state(false);

  const GROUP_ORDER: Kind[] = ["target", "host"];
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

  onMount(() => {
    loadStrings();
    refresh();
    const un = onInstallProgress((p) => {
      for (const row of selectedRows) if (row.id === p.id) progress[key(row)] = p;
      progress = { ...progress };
    });
    return () => {
      un.then((f) => f());
    };
  });
</script>

<div class="shell">
  <header>
    <h1>Xenolith Installer</h1>
    <div class="meta">
      {#if catalog}
        <span>release {catalog.release}</span>
        {#if catalog.nativeId}<span class="native">host {catalog.nativeId}</span>{/if}
      {/if}
    </div>
  </header>

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
    <button class="btn primary" onclick={installSelected} disabled={!selectedCount || busy}>
      {busy ? S["action-installing"] : `${S["action-install"]} (${selectedCount})`}
    </button>
  </footer>

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
</div>

<svelte:window onkeydown={(e) => pending && e.key === "Escape" && (pending = null)} />

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
