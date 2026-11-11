<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let rows = $state([]);
  let shown = $state(null);
  let error = $state('');
  let message = $state('');
  let busy = $state(false);

  async function load() {
    try {
      const r = await api.bak.list();
      rows = (Array.isArray(r.obj) ? r.obj : []).slice().reverse();
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  async function open(row) {
    const r = await api.bak.detail(row.id);
    shown = r.obj || null;
  }

  async function restore(row) {
    if (!confirm($tt('confirmRestore'))) return;
    busy = true;
    error = '';
    message = '';
    const r = await api.bak.restore(row.id);
    if (r.success) {
      message = $tt('restored') + (r.obj && r.obj.output ? '\n' + r.obj.output : '');
      load();
    } else {
      error = r.msg || $tt('failed');
    }
    busy = false;
  }

  function when(row) {
    const t = String(row.time || '');
    const n = Number(t);
    if (!n) return t;
    return new Date(n).toLocaleString();
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('bak')}</h1>
<p class="text-muted-foreground mb-4 text-sm">{$tt('bakHint')}</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

{#if message}
  <Alert.Root class="mb-3"><Alert.Title class="whitespace-pre-wrap">{message}</Alert.Title></Alert.Root>
{/if}

<Card.Root class="overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('uptime')}</Table.Head>
        <Table.Head>bytes</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell>{when(row)}</Table.Cell>
          <Table.Cell class="text-muted-foreground">{row.size}</Table.Cell>
          <Table.Cell class="text-right">
            <Button variant="ghost" size="sm" onclick={() => open(row)}>{$tt('edit')}</Button>
            <Button variant="outline" size="sm" disabled={busy} onclick={() => restore(row)}>{$tt('restore')}</Button>
          </Table.Cell>
        </Table.Row>
      {:else}
        <Table.Row>
          <Table.Cell colspan={3} class="text-muted-foreground py-8 text-center">{$tt('empty')}</Table.Cell>
        </Table.Row>
      {/each}
    </Table.Body>
  </Table.Root>
</Card.Root>

{#if shown}
  <Card.Root class="mt-4">
    <Card.Header class="flex-row items-center justify-between">
      <Card.Title>{when(shown)}</Card.Title>
      <Button variant="ghost" size="sm" onclick={() => (shown = null)}>{$tt('close')}</Button>
    </Card.Header>
    <Card.Content><pre class="code">{shown.content || ''}</pre></Card.Content>
  </Card.Root>
{/if}
