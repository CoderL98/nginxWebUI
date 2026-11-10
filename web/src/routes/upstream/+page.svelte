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
      const r = await api.upstream.list();
      rows = Array.isArray(r.obj) ? r.obj : [];
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    await api.upstream.remove(row.id);
    load();
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('upstream')}</h1>
<p class="text-muted-foreground mb-4 text-sm">后端服务器池。location 的负载均衡类型指向这里。</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<div class="mb-3 flex justify-end">
  <Button href="/upstream/edit">{$tt('add')}</Button>
</div>

<Card.Root class="overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('name2')}</Table.Head>
        <Table.Head>{$tt('strategy')}</Table.Head>
        <Table.Head>{$tt('nodes')}</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell class="font-medium">{row.name}</Table.Cell>
          <Table.Cell class="text-muted-foreground">{row.tactics || 'round robin'}</Table.Cell>
          <Table.Cell class="flex flex-wrap gap-1">
            {#each row.nodes || [] as node}
              <Badge variant={Number(node.enable) ? 'secondary' : 'outline'}>
                {node.server}{node.port ? ':' + node.port : ''}
              </Badge>
            {/each}
          </Table.Cell>
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="ghost" size="sm" href={'/upstream/edit?id=' + encodeURIComponent(row.id)}>{$tt('edit')}</Button>
            <Button variant="destructive" size="sm" onclick={() => drop(row)}>{$tt('del')}</Button>
          </Table.Cell>
        </Table.Row>
      {:else}
        <Table.Row>
          <Table.Cell colspan={4} class="text-muted-foreground py-8 text-center">{$tt('empty')}</Table.Cell>
        </Table.Row>
      {/each}
    </Table.Body>
  </Table.Root>
</Card.Root>
