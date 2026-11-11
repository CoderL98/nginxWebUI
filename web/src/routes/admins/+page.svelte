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
  let me = $state('');
  let busy = $state(false);
  let error = $state('');
  let note = $state('');
  let form = $state({ name: '', pass: '' });

  async function load() {
    error = '';
    try {
      const [list, info] = await Promise.all([api.admin.list(), api.login.info()]);
      rows = Array.isArray(list.obj) ? list.obj : [];
      me = (info.obj || {}).adminId || '';
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  async function add() {
    error = '';
    note = '';
    if (!form.name.trim()) {
      error = $tt('nameRequired');
      return;
    }
    if (!form.pass) {
      error = $tt('passRequired');
      return;
    }
    busy = true;
    try {
      const r = await api.admin.save({ name: form.name.trim(), pass: form.pass });
      if (r.success) {
        form = { name: '', pass: '' };
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

  async function drop(row) {
    if (row.id === me) {
      error = $tt('cannotDeleteSelf');
      return;
    }
    if (!confirm($tt('confirmDel'))) return;
    busy = true;
    error = '';
    note = '';
    try {
      const r = await api.admin.remove(row.id);
      if (r.success) {
        await load();
      } else {
        error = $tt(r.msg) || r.msg || $tt('failed');
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('admins')}</h1>
<p class="text-muted-foreground mb-4 text-sm">{$tt('adminsHint')}</p>

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
    <Card.Title>{$tt('addAdmin')}</Card.Title>
    <Card.Description>{$tt('adminPassHint')}</Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>{$tt('name')}</Label>
      <Input bind:value={form.name} autocomplete="off" />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('pass')}</Label>
      <Input type="password" bind:value={form.pass} autocomplete="new-password" />
    </div>
  </Card.Content>
  <Card.Footer>
    <Button onclick={add} disabled={busy}>{$tt('add')}</Button>
  </Card.Footer>
</Card.Root>

<div class="overflow-x-auto rounded-md border">
  <table class="w-full text-sm">
    <thead class="bg-muted/50 text-left">
      <tr>
        <th class="px-3 py-2 font-medium">{$tt('name')}</th>
        <th class="px-3 py-2 font-medium"></th>
      </tr>
    </thead>
    <tbody>
      {#each rows as row}
        <tr class="border-t">
          <td class="px-3 py-2">
            {row.name}
            {#if row.id === me}
              <span class="text-muted-foreground ml-2 text-xs">{$tt('currentAdmin')}</span>
            {/if}
          </td>
          <td class="px-3 py-2 text-right">
            <Button variant="outline" size="sm" onclick={() => drop(row)} disabled={busy || row.id === me}>
              {$tt('del')}
            </Button>
          </td>
        </tr>
      {:else}
        <tr>
          <td class="text-muted-foreground px-3 py-4" colspan="2">{$tt('empty')}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
