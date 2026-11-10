<script>
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import { flagClass, flagOn } from '$lib/flag.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as NativeSelect from '$lib/components/ui/native-select/index.js';

  let upstream = $state({ name: '', tactics: '', descr: '' });
  let nodes = $state([]);
  let note = $state('');
  let error = $state('');
  let busy = $state(false);

  const tacticsList = [
    { value: '', label: 'round robin' },
    { value: 'ip_hash', label: 'ip_hash' },
    { value: 'least_conn', label: 'least_conn' },
    { value: 'random', label: 'random' },
    { value: 'hash $request_uri consistent', label: 'hash $request_uri consistent' }
  ];

  onMount(async () => {
    const id = $page.url.searchParams.get('id') || '';
    if (!id) {
      addNode();
      return;
    }
    const r = await api.upstream.detail(id);
    const data = r.obj || {};
    if (data.upstream) upstream = { ...upstream, ...data.upstream };
    nodes = data.nodes || [];
  });

  function addNode() {
    nodes = [
      ...nodes,
      { id: 'tmp-' + Math.random().toString(36).slice(2), server: '127.0.0.1', port: 80, weight: 1, enable: 1, status: '', param: '' }
    ];
  }

  async function save() {
    busy = true;
    error = '';
    note = '';
    try {
      const r = await api.upstream.save(
        { ...$state.snapshot(upstream), name: (upstream.name || '').trim(), enable: flagOn(upstream.enable, true) ? 1 : 0 },
        nodes.map((node) => {
          const n = $state.snapshot(node);
          return { ...n, enable: flagOn(n.enable, true) ? 1 : 0, weight: Number(n.weight) };
        })
      );
      if (r.success) {
        note = $tt('saved');
        if (!upstream.id) {
          upstream.id = (r.obj || {}).id;
          history.replaceState(null, '', '/upstream/edit?id=' + encodeURIComponent(upstream.id));
        }
      } else {
        error = r.msg || $tt('failed');
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function drop() {
    if (!upstream.id || !confirm($tt('confirmDel'))) return;
    await api.upstream.remove(upstream.id);
    location.href = '/upstream';
  }
</script>

<div class="mb-4 flex items-baseline gap-3">
  <a href="/upstream" class="text-muted-foreground text-xs">← {$tt('upstream')}</a>
  <h1 class="text-lg font-semibold">{upstream.id ? $tt('edit') : $tt('add')}</h1>
</div>

{#if error}<Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>{/if}
{#if note}<Alert.Root class="mb-3"><Alert.Title>{note}</Alert.Title></Alert.Root>{/if}

<Card.Root class="mb-4">
  <Card.Content class="grid gap-3 pt-6 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>{$tt('name2')}</Label>
      <Input bind:value={upstream.name} placeholder="backend" />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('strategy')}</Label>
      <NativeSelect.Root bind:value={upstream.tactics} class="w-full">
        {#each tacticsList as item}
          <NativeSelect.Option value={item.value}>{item.label}</NativeSelect.Option>
        {/each}
      </NativeSelect.Root>
    </div>
    <div class="grid gap-1.5 sm:col-span-2">
      <Label>{$tt('remark')}</Label>
      <Input bind:value={upstream.descr} />
    </div>
  </Card.Content>
</Card.Root>

<Card.Root class="mb-4">
  <Card.Header class="flex-row items-center justify-between">
    <Card.Title>{$tt('nodes')}</Card.Title>
    <Button variant="outline" size="sm" onclick={addNode}>{$tt('add')}</Button>
  </Card.Header>
  <Card.Content class="grid gap-3">
    {#each nodes as node}
      <div class="grid gap-2 rounded-lg border p-3 sm:grid-cols-4">
        <div class="grid gap-1.5">
          <Label>server</Label>
          <Input bind:value={node.server} />
        </div>
        <div class="grid gap-1.5">
          <Label>port</Label>
          <Input type="number" bind:value={node.port} />
        </div>
        <div class="grid gap-1.5">
          <Label>{$tt('weight')}</Label>
          <Input type="number" bind:value={node.weight} />
        </div>
        <div class="grid gap-1.5">
          <Label>status</Label>
          <NativeSelect.Root bind:value={node.status} class="w-full">
            <NativeSelect.Option value="">up</NativeSelect.Option>
            <NativeSelect.Option value="down">down</NativeSelect.Option>
            <NativeSelect.Option value="backup">backup</NativeSelect.Option>
          </NativeSelect.Root>
        </div>
        <label class="{flagClass(flagOn(node.enable, true))} sm:col-span-3 w-fit">
          <Switch checked={flagOn(node.enable, true)} onCheckedChange={(v) => (node.enable = v ? 1 : 0)} />
          {$tt('enable')}
        </label>
        <Button variant="destructive" size="sm" onclick={() => (nodes = nodes.filter((n) => n !== node))}>{$tt('del')}</Button>
      </div>
    {:else}
      <p class="text-muted-foreground text-sm">{$tt('empty')}</p>
    {/each}
  </Card.Content>
</Card.Root>

<div class="flex gap-2">
  <Button onclick={save} disabled={busy}>{$tt('save')}</Button>
  {#if upstream.id}<Button variant="destructive" onclick={drop}>{$tt('del')}</Button>{/if}
</div>
