<script>
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';

  let {
    columns = [],
    fields = [],
    load,
    save,
    remove,
    blank = {},
    intro = ''
  } = $props();

  let rows = $state([]);
  let editing = $state(null);
  let error = $state('');
  let note = $state('');
  let busy = $state(false);
  let open = $state(false);

  async function refresh() {
    try {
      const r = await load();
      const value = r && r.obj !== undefined ? r.obj : r;
      rows = Array.isArray(value) ? value : value && value.records ? value.records : [];
      if (r && r.success === false) error = r.msg;
    } catch (e) {
      error = String(e && e.message ? e.message : e);
    }
  }

  refresh();

  function startNew() {
    error = '';
    note = '';
    editing = { ...blank };
    open = true;
  }

  function startEdit(row) {
    error = '';
    note = '';
    editing = { ...row };
    open = true;
  }

  async function submit() {
    busy = true;
    error = '';
    try {
      const r = await save(editing);
      if (r.success) {
        note = $tt('saved');
        editing = null;
        open = false;
        await refresh();
      } else {
        error = r.msg || $tt('failed');
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    try {
      const r = await remove(row.id);
      if (r.success) await refresh();
      else error = r.msg || $tt('failed');
    } catch {
      error = $tt('failed');
    }
  }

  function labelOf(key) {
    return $tt(key) === key ? key : $tt(key);
  }
</script>

{#if intro}<p class="text-muted-foreground mb-3 text-sm">{intro}</p>{/if}
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
        {#each columns as col}<Table.Head>{labelOf(col.label)}</Table.Head>{/each}
        <Table.Head class="w-1"></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          {#each columns as col}
            <Table.Cell>
              {#if col.key === 'value' || col.key === 'ip' || col.key === 'pass' || col.key === 'pem'}
                <code class="text-muted-foreground font-mono text-xs">{String(row[col.key] ?? '')}</code>
              {:else}
                {String(row[col.key] ?? '')}
              {/if}
            </Table.Cell>
          {/each}
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="ghost" size="sm" onclick={() => startEdit(row)}>{$tt('edit')}</Button>
            <Button variant="destructive" size="sm" onclick={() => drop(row)}>{$tt('del')}</Button>
          </Table.Cell>
        </Table.Row>
      {:else}
        <Table.Row>
          <Table.Cell colspan={columns.length + 1} class="text-muted-foreground py-8 text-center">
            {$tt('empty')}
          </Table.Cell>
        </Table.Row>
      {/each}
    </Table.Body>
  </Table.Root>
</Card.Root>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{editing?.id ? $tt('edit') : $tt('add')}</Dialog.Title>
    </Dialog.Header>
    {#if editing}
      <div class="grid gap-3">
        {#each fields as field}
          <div class="grid gap-1.5">
            <Label>{labelOf(field.label)}</Label>
            {#if field.type === 'area'}
              <Textarea rows={field.rows || 4} bind:value={editing[field.key]} />
            {:else if field.type === 'number'}
              <Input type="number" bind:value={editing[field.key]} />
            {:else}
              <Input bind:value={editing[field.key]} />
            {/if}
          </div>
        {/each}
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{$tt('cancel')}</Button>
      <Button onclick={submit} disabled={busy}>{$tt('save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
