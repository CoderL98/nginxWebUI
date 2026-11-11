<script>
  import { onMount } from 'svelte';
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

  function statusLabel(status) {
    if (status === 'online') return $tt('nodeOnline');
    if (status === 'pending') return $tt('nodePending');
    if (status === 'revoked') return $tt('nodeRevoked');
    return $tt('nodeOffline');
  }

  function seenText(ms) {
    const n = Number(ms || 0);
    if (!n) return '-';
    return new Date(n).toLocaleString();
  }

  async function load() {
    error = '';
    try {
      const r = await api.node.list();
      rows = Array.isArray(r.obj) ? r.obj : [];
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

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
      const r = await api.node.save({ name: name.trim() });
      if (r.success) {
        issued = r.obj || {};
        name = '';
        note = $tt('saved');
        await load();
      } else {
        error = $tt(r.msg) || r.msg || $tt('failed');
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
      const r = await api.node.token(row.id);
      if (r.success) {
        issued = { ...row, ...(r.obj || {}) };
      } else {
        error = $tt(r.msg) || r.msg || $tt('failed');
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function rename(row) {
    const next = prompt($tt('name'), row.name);
    if (next == null || !next.trim() || next.trim() === row.name) return;
    busy = true;
    try {
      const r = await api.node.save({ id: row.id, name: next.trim() });
      if (!r.success) error = $tt(r.msg) || r.msg || $tt('failed');
      await load();
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    busy = true;
    try {
      const r = await api.node.remove(row.id);
      if (!r.success) error = $tt(r.msg) || r.msg || $tt('failed');
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
  <p class="text-muted-foreground mb-4 text-sm">{note}</p>
{/if}

<Card.Root class="mb-4">
  <Card.Header>
    <Card.Title>{$tt('addNode')}</Card.Title>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-[1fr_auto] sm:items-end">
    <div class="grid gap-1.5">
      <Label>{$tt('name')}</Label>
      <Input bind:value={name} autocomplete="off" />
    </div>
    <Button onclick={add} disabled={busy}>{$tt('add')}</Button>
  </Card.Content>
</Card.Root>

{#if issued && (issued.token || issued.installCommand)}
  <Card.Root class="mb-4">
    <Card.Header>
      <Card.Title>{$tt('installCommand')}</Card.Title>
      <Card.Description>{$tt('nodeToken')}: {issued.token}</Card.Description>
    </Card.Header>
    <Card.Content>
      <pre class="bg-muted overflow-x-auto rounded-md p-3 text-xs">{issued.installCommand}</pre>
    </Card.Content>
    <Card.Footer>
      <Button variant="outline" onclick={() => copyText(issued.installCommand)}>{$tt('copyCommand')}</Button>
    </Card.Footer>
  </Card.Root>
{/if}

<div class="overflow-x-auto rounded-md border">
  <table class="w-full text-sm">
    <thead class="bg-muted/50 text-left">
      <tr>
        <th class="px-3 py-2 font-medium">{$tt('name')}</th>
        <th class="px-3 py-2 font-medium">{$tt('type')}</th>
        <th class="px-3 py-2 font-medium"></th>
        <th class="px-3 py-2 font-medium">{$tt('lastSeen')}</th>
        <th class="px-3 py-2 font-medium"></th>
      </tr>
    </thead>
    <tbody>
      {#each rows as row}
        <tr class="border-t">
          <td class="px-3 py-2">{row.name}</td>
          <td class="px-3 py-2">{row.kind === 'local' ? $tt('currentNode') : row.hostname || row.os || 'agent'}</td>
          <td class="px-3 py-2">{statusLabel(row.status)}</td>
          <td class="px-3 py-2">{row.kind === 'local' ? '-' : seenText(row.lastSeen)}</td>
          <td class="px-3 py-2 text-right">
            {#if row.kind !== 'local'}
              <div class="flex flex-wrap justify-end gap-2">
                <Button variant="outline" size="sm" disabled={busy} onclick={() => rename(row)}>{$tt('edit')}</Button>
                <Button variant="outline" size="sm" disabled={busy} onclick={() => token(row)}>{$tt('newToken')}</Button>
                <Button variant="outline" size="sm" disabled={busy} onclick={() => drop(row)}>{$tt('revokeNode')}</Button>
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
