<script>
  import { onMount } from 'svelte';
  import { api, isRemote } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as NativeSelect from '$lib/components/ui/native-select/index.js';

  let data = $state({});
  let files = $state([]);
  let shown = $state('main');
  let message = $state('');
  let error = $state('');
  let busy = $state(false);
  let editing = $state(false);
  let draft = $state('');
  const remote = isRemote();

  /** `obj.success` (current) or `obj.ok` (older agents). */
  function passed(r) {
    const out = (r && r.obj) || {};
    if (typeof out.success === 'boolean') return out.success;
    if (typeof out.ok === 'boolean') return out.ok;
    return false;
  }

  async function load() {
    try {
      const r = await api.conf.show();
      if (r.success === false) error = $tt(r.msg) || r.msg;
      data = r.obj || {};
      files = data.files || [];
      draft = data.conf || '';
    } catch {
      error = $tt('failed');
    }
  }

  onMount(load);

  let text = $derived(shown === 'main' ? draft : files.find((f) => f.name === shown)?.conf || '');

  async function check() {
    busy = true;
    message = '';
    const r = await api.conf.check();
    const out = r.obj || {};
    message = r.success === false ? $tt(r.msg) || r.msg : (passed(r) ? '✓\n' : '✗\n') + (out.output || '');
    busy = false;
  }

  async function reload() {
    busy = true;
    message = '';
    const r = await api.conf.reload();
    const out = r.obj || {};
    message = r.success === false ? $tt(r.msg) || r.msg : (out.output || '') + (passed(r) ? '\n✓' : '\n✗');
    busy = false;
  }

  async function writeGenerated() {
    busy = true;
    message = '';
    const r = await api.conf.write();
    if (r.success) {
      message = $tt('saved') + '\n' + JSON.stringify(r.obj || {}, null, 2);
    } else if (r.msg === 'refuseOverwriteNginxConf') {
      message = $tt('refuseOverwrite');
    } else {
      message = r.msg || $tt('failed');
    }
    busy = false;
    load();
  }

  async function saveFiles() {
    busy = true;
    message = '';
    const r = await api.conf.replace(draft);
    message = r.success ? $tt('saved') : r.msg || $tt('failed');
    busy = false;
    editing = false;
    load();
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('conf')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  {$tt('confLiveHint')} {data.reloadCmd || 'sudo systemctl reload nginx'}。
</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<Card.Root class="mb-4">
  <Card.Content class="grid gap-3 pt-6 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>nginx.conf</Label>
      <Input value={data.nginxPath || ''} readonly />
    </div>
    <div class="grid gap-1.5">
      <Label>nginx prefix</Label>
      <Input value={data.nginxDir || ''} readonly />
    </div>
    <details class="sm:col-span-2">
      <summary class="text-muted-foreground cursor-pointer text-xs">{$tt('effectivePaths')}</summary>
      <dl class="mt-2 grid gap-x-4 gap-y-1 font-mono text-xs sm:grid-cols-[auto_1fr]">
        <dt class="text-muted-foreground">sites-available</dt><dd>{data.sitesAvailable || '—'}</dd>
        <dt class="text-muted-foreground">sites-enabled</dt><dd>{data.sitesEnabled || '—'}</dd>
        <dt class="text-muted-foreground">ssl</dt><dd>{data.sslDir || '—'}</dd>
        <dt class="text-muted-foreground">ssl-params</dt><dd>{data.sslParams || '—'}</dd>
        <dt class="text-muted-foreground">{$tt('webRoot')}</dt><dd>{data.webRoot || '—'}</dd>
        <dt class="text-muted-foreground">{$tt('reloadCmd')}</dt><dd>{data.reloadCmd || '—'}</dd>
        <dt class="text-muted-foreground">nginx</dt><dd>{data.nginxExe || '—'}</dd>
      </dl>
    </details>
    <div class="flex flex-wrap items-center gap-2 sm:col-span-2">
      <Badge variant={data.running ? 'default' : 'destructive'}>
        {data.running ? $tt('running') : $tt('stopped')}
      </Badge>
      <span class="text-muted-foreground text-xs">conf.d: {data.decompose ? $tt('yes') : $tt('no')}</span>
    </div>
    <div class="flex flex-wrap gap-2 sm:col-span-2">
      <Button variant="outline" onclick={check} disabled={busy}>{$tt('check')}</Button>
      <Button onclick={reload} disabled={busy}>{$tt('reload')}</Button>
      {#if remote}
        <span class="text-muted-foreground self-center text-xs">{$tt('remoteConfHint')}</span>
      {:else}
      <Button variant="outline" onclick={writeGenerated} disabled={busy || data.canOverwrite === false}>{$tt('writeConf')}</Button>
      {/if}
      {#if remote}
        <!-- whole-file edits stay on the panel host -->
      {:else if editing}
        <Button onclick={saveFiles} disabled={busy}>{$tt('save')}</Button>
        <Button variant="ghost" onclick={() => (editing = false)}>{$tt('cancel')}</Button>
      {:else}
        <Button variant="outline" onclick={() => (editing = true)}>{$tt('edit')}</Button>
      {/if}
    </div>
    {#if message}<pre class="code sm:col-span-2 max-h-48">{message}</pre>{/if}
  </Card.Content>
</Card.Root>

<Card.Root>
  <Card.Header class="flex-row items-center justify-between">
    <Card.Title>{shown === 'main' ? 'nginx.conf' : shown}</Card.Title>
    {#if files.length}
      <NativeSelect.Root bind:value={shown}>
        <NativeSelect.Option value="main">nginx.conf</NativeSelect.Option>
        {#each files as file}
          <NativeSelect.Option value={file.name}>{file.name}</NativeSelect.Option>
        {/each}
      </NativeSelect.Root>
    {/if}
  </Card.Header>
  <Card.Content>
    {#if editing && shown === 'main'}
      <Textarea rows={22} bind:value={draft} spellcheck="false" class="font-mono text-xs" />
    {:else}
      <pre class="code">{text || '—'}</pre>
    {/if}
  </Card.Content>
</Card.Root>
