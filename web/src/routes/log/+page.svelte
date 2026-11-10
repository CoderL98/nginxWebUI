<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as NativeSelect from '$lib/components/ui/native-select/index.js';

  let dir = $state('');
  let names = $state([]);
  let current = $state('');
  let text = $state('');
  let lines = $state(200);
  let error = $state('');
  let busy = $state(false);

  async function loadNames() {
    try {
      const r = await api.log.names();
      const data = r.obj || {};
      dir = data.dir || '';
      names = data.names || [];
      if (!current && names.length) current = names[0];
      if (current) await loadTail();
    } catch {
      error = $tt('failed');
    }
  }

  async function loadTail() {
    if (!current) return;
    busy = true;
    try {
      const r = await api.log.tail(current, lines);
      text = (r.obj || {}).text || '';
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  onMount(loadNames);
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('log')}</h1>
<p class="text-muted-foreground mb-4 text-sm">读取本机 nginx 日志文件末尾，不把整份日志载入内存。</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<Card.Root class="mb-4">
  <Card.Content class="flex flex-wrap items-end gap-3 pt-6">
    <div class="grid min-w-40 gap-1.5">
      <Label>{$tt('type')}</Label>
      <NativeSelect.Root bind:value={current} onchange={loadTail}>
        {#each names as name}
          <NativeSelect.Option value={name}>{name}</NativeSelect.Option>
        {/each}
      </NativeSelect.Root>
    </div>
    <div class="grid min-w-28 gap-1.5">
      <Label>lines</Label>
      <NativeSelect.Root bind:value={lines} onchange={loadTail}>
        <NativeSelect.Option value={100}>100</NativeSelect.Option>
        <NativeSelect.Option value={200}>200</NativeSelect.Option>
        <NativeSelect.Option value={500}>500</NativeSelect.Option>
        <NativeSelect.Option value={1000}>1000</NativeSelect.Option>
      </NativeSelect.Root>
    </div>
    <Button variant="outline" onclick={loadTail} disabled={busy}>↻</Button>
    <p class="text-muted-foreground w-full font-mono text-xs">{dir}</p>
  </Card.Content>
</Card.Root>

<Card.Root>
  <Card.Content class="pt-6">
    <pre class="code">{text || (busy ? '…' : '—')}</pre>
  </Card.Content>
</Card.Root>
