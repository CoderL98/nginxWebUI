<script>
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let rows = $state([]);
  let busy = $state(false);
  let error = $state('');
  let note = $state('');
  let name = $state('');
  let issued = $state(null);
  let panelUrl = $state('');
  let detectedUrl = $state('');
  let renaming = $state('');
  let renameValue = $state('');
  let timer = null;

  function statusLabel(status) {
    if (status === 'online') return $tt('nodeOnline');
    if (status === 'pending') return $tt('nodePending');
    if (status === 'revoked') return $tt('nodeRevoked');
    return $tt('nodeOffline');
  }

  function statusClass(status) {
    if (status === 'online') return 'text-emerald-600 dark:text-emerald-400';
    if (status === 'revoked') return 'text-muted-foreground';
    return 'text-destructive';
  }

  function seenText(ms) {
    const n = Number(ms || 0);
    if (!n) return '-';
    return new Date(n).toLocaleString();
  }

  function msg(r) {
    return (r && r.msg && $tt(r.msg)) || $tt('failed');
  }

  /** The install command for the token on screen, using the URL in the field. */
  function commandFor(token) {
    const base = (panelUrl.trim() || detectedUrl || 'https://panel.example').replace(/\/+$/, '');
    return `curl -fsSL ${base}/agent/install.sh | sh -s -- --url ${base} --token ${token}`;
  }

  function urlWarning() {
    const base = (panelUrl.trim() || detectedUrl || '').toLowerCase();
    const host = base.replace(/^https?:\/\//, '').split(/[/?]/)[0].replace(/:\d+$/, '');
    if (host === 'localhost' || host.startsWith('127.') || host === '[::1]') return 'installUrlLoopback';
    if (base.startsWith('http://')) return 'installUrlInsecure';
    return '';
  }

  async function load() {
    try {
      const r = await api.node.list();
      if (r.success === false) {
        error = msg(r);
        return;
      }
      rows = Array.isArray(r.obj) ? r.obj : [];
    } catch {
      error = $tt('failed');
    }
  }

  async function loadConfig() {
    try {
      const r = await api.node.config();
      const obj = r.obj || {};
      panelUrl = obj.panelUrl || '';
      detectedUrl = obj.detected || '';
    } catch {
      /* optional */
    }
  }

  onMount(() => {
    load();
    loadConfig();
    // keep online/offline current without a manual refresh
    timer = setInterval(load, 10000);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  async function saveUrl() {
    error = '';
    note = '';
    const r = await api.node.saveConfig(panelUrl.trim());
    if (r.success) {
      note = $tt('saved');
      panelUrl = (r.obj || {}).panelUrl || '';
    } else {
      error = msg(r);
    }
  }

  async function add() {
    error = '';
    note = '';
    issued = null;
    if (!name.trim()) {
      error = $tt('nameRequired');
      return;
    }
    busy = true;
    try {
      const r = await api.node.save({ name: name.trim(), panelUrl: panelUrl.trim() });
      if (r.success) {
        issued = r.obj || {};
        name = '';
        note = $tt('saved');
        await load();
      } else {
        error = msg(r);
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function token(row) {
    busy = true;
    error = '';
    issued = null;
    try {
      const r = await api.node.token(row.id, panelUrl.trim());
      if (r.success) {
        issued = { ...row, ...(r.obj || {}), rotated: row.joined };
      } else {
        error = msg(r);
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  function startRename(row) {
    renaming = row.id;
    renameValue = row.name;
  }

  async function rename(row) {
    const next = renameValue.trim();
    renaming = '';
    if (!next || next === row.name) return;
    busy = true;
    try {
      const r = await api.node.save({ id: row.id, name: next });
      if (!r.success) error = msg(r);
      await load();
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function revoke(row) {
    if (!confirm($tt('confirmRevoke'))) return;
    busy = true;
    try {
      const r = await api.node.revoke(row.id);
      if (!r.success) error = msg(r);
      if (issued && issued.id === row.id) issued = null;
      await load();
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function drop(row) {
    if (!confirm($tt('confirmDeleteNode'))) return;
    busy = true;
    try {
      const r = await api.node.remove(row.id);
      if (!r.success) error = msg(r);
      if (issued && issued.id === row.id) issued = null;
      await load();
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      note = $tt('copied');
    } catch {
      error = $tt('failed');
    }
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('nodes')}</h1>
<p class="text-muted-foreground mb-4 text-sm">{$tt('nodesHint')}</p>

{#if error}
  <Alert.Root class="mb-4" variant="destructive">
    <Alert.Description>{error}</Alert.Description>
  </Alert.Root>
{/if}
{#if note}
  <p class="text-muted-foreground mb-4 text-sm" role="status">{note}</p>
{/if}

<Card.Root class="mb-4">
  <Card.Header>
    <Card.Title>{$tt('addNode')}</Card.Title>
  </Card.Header>
  <Card.Content class="grid gap-3">
    <div class="grid gap-3 sm:grid-cols-[1fr_auto] sm:items-end">
      <div class="grid gap-1.5">
        <Label for="node-panel-url">{$tt('panelUrl')}</Label>
        <Input id="node-panel-url" bind:value={panelUrl} placeholder={detectedUrl || 'https://panel.example'} autocomplete="off" />
      </div>
      <Button variant="outline" onclick={saveUrl} disabled={busy}>{$tt('save')}</Button>
    </div>
    <p class="text-muted-foreground text-xs">{$tt('panelUrlHint')}</p>
    {#if urlWarning()}
      <p class="text-destructive text-xs" role="alert">{$tt(urlWarning())}</p>
    {/if}
    <div class="grid gap-3 sm:grid-cols-[1fr_auto] sm:items-end">
      <div class="grid gap-1.5">
        <Label for="node-name">{$tt('name2')}</Label>
        <Input id="node-name" bind:value={name} autocomplete="off" />
      </div>
      <Button onclick={add} disabled={busy}>{$tt('add')}</Button>
    </div>
  </Card.Content>
</Card.Root>

{#if issued && issued.token}
  <Card.Root class="mb-4">
    <Card.Header>
      <Card.Title>{$tt('installCommand')} · {issued.name}</Card.Title>
      <Card.Description>{$tt('nodeToken')}: <code class="font-mono">{issued.token}</code></Card.Description>
    </Card.Header>
    <Card.Content class="grid gap-2">
      <pre class="bg-muted overflow-x-auto rounded-md p-3 text-xs" aria-label={$tt('installCommand')}>{commandFor(issued.token)}</pre>
      {#if urlWarning()}
        <p class="text-destructive text-xs" role="alert">{$tt(urlWarning())}</p>
      {/if}
      <p class="text-muted-foreground text-xs">{$tt('installModeHint')}</p>
      {#if issued.rotated}
        <p class="text-muted-foreground text-xs">{$tt('tokenRotateHint')}</p>
      {/if}
    </Card.Content>
    <Card.Footer>
      <Button variant="outline" onclick={() => copyText(commandFor(issued.token))}>{$tt('copyCommand')}</Button>
    </Card.Footer>
  </Card.Root>
{/if}

<div class="overflow-x-auto rounded-md border">
  <table class="w-full text-sm">
    <caption class="sr-only">{$tt('nodes')}</caption>
    <thead class="bg-muted/50 text-left">
      <tr>
        <th scope="col" class="px-3 py-2 font-medium">{$tt('name2')}</th>
        <th scope="col" class="px-3 py-2 font-medium">{$tt('type')}</th>
        <th scope="col" class="px-3 py-2 font-medium">{$tt('nodeStatus')}</th>
        <th scope="col" class="px-3 py-2 font-medium">{$tt('lastSeen')}</th>
        <th scope="col" class="px-3 py-2 text-right font-medium">{$tt('nodeActions')}</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as row}
        <tr class="border-t">
          <td class="px-3 py-2">
            {#if renaming === row.id}
              <form class="flex gap-2" onsubmit={(e) => { e.preventDefault(); rename(row); }}>
                <Input bind:value={renameValue} aria-label={$tt('renameNode')} class="h-8" />
                <Button type="submit" size="sm">{$tt('save')}</Button>
                <Button type="button" size="sm" variant="ghost" onclick={() => (renaming = '')}>{$tt('cancel')}</Button>
              </form>
            {:else}
              {row.name}
            {/if}
          </td>
          <td class="px-3 py-2">{row.kind === 'local' ? $tt('currentNode') : [row.hostname, row.os, row.agentVersion && 'v' + row.agentVersion].filter(Boolean).join(' · ') || 'agent'}</td>
          <td class="px-3 py-2">
            <span class={statusClass(row.status)}>{statusLabel(row.status)}</span>
            {#if row.conflict}
              <span class="text-destructive block text-xs" role="alert">{$tt('nodeConflict')}</span>
            {/if}
          </td>
          <td class="px-3 py-2">{row.kind === 'local' ? '-' : seenText(row.lastSeen)}</td>
          <td class="px-3 py-2 text-right">
            {#if row.kind !== 'local'}
              <div class="flex flex-wrap justify-end gap-2">
                <Button variant="outline" size="sm" disabled={busy} aria-label={`${$tt('renameNode')} ${row.name}`} onclick={() => startRename(row)}>{$tt('renameNode')}</Button>
                {#if row.status !== 'revoked'}
                  <Button variant="outline" size="sm" disabled={busy} aria-label={`${$tt('newToken')} ${row.name}`} onclick={() => token(row)}>{$tt('newToken')}</Button>
                  <Button variant="outline" size="sm" disabled={busy} aria-label={`${$tt('revokeNode')} ${row.name}`} onclick={() => revoke(row)}>{$tt('revokeNode')}</Button>
                {/if}
                <Button variant="destructive" size="sm" disabled={busy} aria-label={`${$tt('deleteNode')} ${row.name}`} onclick={() => drop(row)}>{$tt('deleteNode')}</Button>
              </div>
            {/if}
          </td>
        </tr>
      {:else}
        <tr>
          <td class="text-muted-foreground px-3 py-4" colspan="5">{$tt('empty')}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
