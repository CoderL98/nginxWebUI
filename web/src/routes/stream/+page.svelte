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

  async function load() {
    try {
      const r = await api.server.list({ page: 1, limit: 500 });
      const all = (r.obj || {}).records || [];
      rows = all.filter((row) => Number(row.proxyType || 0) !== 0);
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    await api.server.remove(row.id);
    load();
  }

  function kindOf(row) {
    return Number(row.proxyType) === 2 ? 'UDP' : 'TCP';
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('stream')}</h1>
<p class="text-muted-foreground mb-4 text-sm">stream block：TCP/UDP 转发到上游池。</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<div class="mb-3 flex justify-end">
  <Button href="/server/edit?kind=stream">{$tt('add')}</Button>
</div>

<Card.Root class="overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('type')}</Table.Head>
        <Table.Head>{$tt('listen')}</Table.Head>
        <Table.Head>{$tt('upstream')}</Table.Head>
        <Table.Head>{$tt('remark')}</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell><Badge variant="secondary">{kindOf(row)}</Badge></Table.Cell>
          <Table.Cell><code class="font-mono text-xs">{row.listen}</code></Table.Cell>
          <Table.Cell>{row.proxyUpstreamId ? '→ pool' : '-'}</Table.Cell>
          <Table.Cell class="text-muted-foreground text-xs">{row.descr || ''}</Table.Cell>
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="ghost" size="sm" href={'/server/edit?id=' + encodeURIComponent(row.id)}>{$tt('edit')}</Button>
            <Button variant="destructive" size="sm" onclick={() => drop(row)}>{$tt('del')}</Button>
          </Table.Cell>
        </Table.Row>
      {:else}
        <Table.Row>
          <Table.Cell colspan={5} class="text-muted-foreground py-8 text-center">{$tt('empty')}</Table.Cell>
        </Table.Row>
      {/each}
    </Table.Body>
  </Table.Root>
</Card.Root>
