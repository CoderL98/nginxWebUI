<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let rows = $state([]);
  let error = $state('');
  let busy = $state(false);

  function typeLabel(row) {
    const kind = row.kind || row.mode;
    if (kind === 'forward') return 'forwardProxy';
    if (kind === 'reverse' || kind === 'proxy') return 'reverseProxy';
    const locs = row.locations || [];
    if (locs.length) {
      const t = locs[0].type;
      if (t === 'proxy' || t === '0') return 'reverseProxy';
      if (t === 'upstream' || t === '2') return 'upstream';
      if (t === 'redirect' || t === '4') return 'locRedirect';
      return 'staticSite';
    }
    if (row.mode === 'substatic') return 'subStatic';
    return 'staticSite';
  }

  function targetOf(row) {
    if ((row.kind || row.mode) === 'forward') {
      return ':' + (row.listen || '8888');
    }
    const locs = row.locations || [];
    if (locs.length) {
      return locs
        .map((l) => {
          const p = l.path || '/';
          if (l.type === 'proxy' || l.type === '0') return p + ' → ' + (l.proxy || row.proxy || '');
          if (l.type === 'upstream' || l.type === '2') return p + ' → ' + (l.upstream || '');
          if (l.type === 'redirect' || l.type === '4') return p + ' → ' + (l.returnUrl || '');
          return p + ' → ' + (l.root || row.root || '');
        })
        .join('; ');
    }
    return row.mode === 'proxy' || row.kind === 'reverse' ? row.proxy : row.root || '';
  }

  async function load() {
    try {
      const r = await api.site.list();
      rows = Array.isArray(r.obj) ? r.obj : [];
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  async function apply(row) {
    busy = true;
    error = '';
    const r = await api.site.apply(row.id);
    if (!r.success) error = r.msg || $tt('failed');
    busy = false;
    load();
  }

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    await api.site.remove(row.id);
    load();
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('site')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  {$tt('compatHint')}
</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<div class="mb-3 flex flex-wrap justify-end gap-2">
  <Button variant="outline" href="/server/edit?kind=static">{$tt('addStatic')}</Button>
  <Button variant="outline" href="/server/edit?kind=reverse">{$tt('addReverse')}</Button>
  <Button variant="outline" href="/server/edit?kind=forward">{$tt('addForward')}</Button>
</div>

<Card.Root class="overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('domain')}</Table.Head>
        <Table.Head>{$tt('type')}</Table.Head>
        <Table.Head>{$tt('ssl')}</Table.Head>
        <Table.Head>{$tt('target')}</Table.Head>
        <Table.Head>{$tt('cert')}</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell class="font-medium">
            {row.domain}
            {#if row.imported || row.fromDisk}
              <Badge variant="outline" class="ml-1">{$tt('imported')}</Badge>
            {/if}
          </Table.Cell>
          <Table.Cell>
            <Badge variant="secondary">{$tt(typeLabel(row))}</Badge>
          </Table.Cell>
          <Table.Cell>{row.ssl === false || row.ssl === 0 || row.ssl === 'false' ? $tt('no') : $tt('yes')}</Table.Cell>
          <Table.Cell class="text-muted-foreground max-w-xs truncate font-mono text-xs">{targetOf(row)}</Table.Cell>
          <Table.Cell class="font-mono text-xs">{row.certName || 'auto'}</Table.Cell>
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="outline" size="sm" disabled={busy} onclick={() => apply(row)}>
              {row.fromDisk ? $tt('reload') : $tt('applySite')}
            </Button>
            <Button variant="ghost" size="sm" href={'/server/edit?id=' + encodeURIComponent(row.id)}>{$tt('edit')}</Button>
            <Button variant="destructive" size="sm" onclick={() => drop(row)}>{$tt('del')}</Button>
          </Table.Cell>
        </Table.Row>
      {:else}
        <Table.Row>
          <Table.Cell colspan={6} class="text-muted-foreground py-8 text-center">{$tt('empty')}</Table.Cell>
        </Table.Row>
      {/each}
    </Table.Body>
  </Table.Root>
</Card.Root>
