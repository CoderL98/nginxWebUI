<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';

  let rows = $state([]);
  let webRoot = $state('/var/www');
  let error = $state('');
  let note = $state('');
  let busy = $state(false);
  let open = $state(false);
  let form = $state({ dir: '', descr: '', file: null });

  async function load() {
    try {
      const r = await api.www.list();
      const obj = r.obj || {};
      rows = Array.isArray(obj.list) ? obj.list : Array.isArray(r.obj) ? r.obj : [];
      webRoot = obj.webRoot || webRoot;
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  function startNew() {
    form = { dir: webRoot + '/', descr: '', file: null };
    open = true;
    error = '';
  }

  async function submit() {
    busy = true;
    error = '';
    const r = await api.www.upload(form.dir, form.file, form.descr);
    if (r.success) {
      open = false;
      note = $tt('saved');
      await load();
    } else {
      error = r.msg || $tt('failed');
    }
    busy = false;
  }

  async function clean(row) {
    if (!confirm($tt('cleanWww') + ' ' + row.dir + ' ?')) return;
    await api.www.clean(row.id);
    load();
  }

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    await api.www.remove(row.id);
    load();
  }

  function copy(text) {
    navigator.clipboard.writeText(text);
    note = $tt('copied');
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('www')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  {$tt('zipTips')} · {$tt('root')}: {webRoot}
</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}
{#if note}
  <Alert.Root class="mb-3"><Alert.Title>{note}</Alert.Title></Alert.Root>
{/if}

<div class="mb-3 flex justify-end">
  <Button onclick={startNew}>{$tt('add')}</Button>
</div>

<Card.Root class="overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('path')}</Table.Head>
        <Table.Head>{$tt('files')}</Table.Head>
        <Table.Head>{$tt('remark')}</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell class="font-mono text-xs">
            {row.dir || row.path}
            <Button variant="ghost" size="sm" onclick={() => copy(row.dir || row.path)}>{$tt('copy')}</Button>
          </Table.Cell>
          <Table.Cell>{row.files ?? 0}</Table.Cell>
          <Table.Cell>{row.descr || ''}</Table.Cell>
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="outline" size="sm" onclick={() => clean(row)}>{$tt('cleanWww')}</Button>
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

<Dialog.Root bind:open>
  <Dialog.Content>
    <Dialog.Header>
      <Dialog.Title>{$tt('upload')}</Dialog.Title>
      <Dialog.Description>{$tt('zipTips')}</Dialog.Description>
    </Dialog.Header>
    <div class="grid gap-3">
      <div class="grid gap-1.5">
        <Label>{$tt('root')}</Label>
        <Input bind:value={form.dir} placeholder={webRoot + '/example.com'} />
      </div>
      <div class="grid gap-1.5">
        <Label>{$tt('remark')}</Label>
        <Input bind:value={form.descr} />
      </div>
      <div class="grid gap-1.5">
        <Label>{$tt('uploadZip')}</Label>
        <Input type="file" accept=".zip,application/zip" onchange={(e) => (form.file = e.currentTarget.files?.[0] || null)} />
      </div>
    </div>
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{$tt('cancel')}</Button>
      <Button onclick={submit} disabled={busy || !form.dir}>{$tt('save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
