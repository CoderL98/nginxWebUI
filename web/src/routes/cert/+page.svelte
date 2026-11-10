<script>
  import { onMount, tick } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import * as Table from '$lib/components/ui/table/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as Dialog from '$lib/components/ui/dialog/index.js';
  import * as NativeSelect from '$lib/components/ui/native-select/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';

  let rows = $state([]);
  let sslFiles = $state([]);
  let acme = $state({ found: false, path: '', hasCf: false, hasGcore: false, sslDir: '' });
  let canInstall = $state(false);
  let error = $state('');
  let log = $state('');
  let busy = $state(false);
  let open = $state(false);
  let editing = $state(null);
  let logEl = $state(null);
  let jobKind = $state('');
  let jobCertId = $state('');

  const blank = {
    domain: '',
    kind: 'dns_cf',
    force: false,
    descr: '',
    pemContent: '',
    keyContent: ''
  };

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  async function scrollLog() {
    await tick();
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  }

  async function load() {
    try {
      const r = await api.cert.list();
      const obj = r.obj || {};
      rows = Array.isArray(obj.certs) ? obj.certs : [];
      sslFiles = obj.sslFiles || [];
      acme = obj.acme || acme;
      canInstall = !!obj.canInstall;
      return obj.job || null;
    } catch {
      error = $tt('failed');
      return null;
    }
  }

  async function followJob() {
    for (;;) {
      const r = await api.cert.progress();
      const job = r.obj || {};
      log = job.output || '';
      jobKind = job.kind || '';
      jobCertId = job.certId || '';
      await scrollLog();
      if (!job.running) {
        error = job.success === false ? $tt('failed') : '';
        await load();
        return job;
      }
      await sleep(350);
    }
  }

  async function startAndFollow(start, placeholder) {
    busy = true;
    error = '';
    log = placeholder || $tt('issuing');
    jobKind = '';
    jobCertId = '';
    const r = await start();
    const job = r.obj || {};
    if (job.output) log = job.output;
    if (r.msg === 'jobRunning') error = $tt('jobRunning');
    if (job.running || r.msg === 'jobRunning') {
      await followJob();
    } else {
      log = job.output || r.msg || '';
      if (!r.success) error = r.msg || $tt('failed');
      await load();
    }
    busy = false;
  }

  onMount(async () => {
    const job = await load();
    if (job && job.running) {
      busy = true;
      log = job.output || $tt('issuing');
      jobKind = job.kind || '';
      jobCertId = job.certId || '';
      await followJob();
      busy = false;
    }
  });

  function startNew() {
    editing = { ...blank };
    open = true;
    error = '';
  }

  function startEdit(row) {
    editing = { ...blank, ...row, force: false };
    open = true;
    error = '';
  }

  async function save() {
    busy = true;
    error = '';
    const r = await api.cert.save(editing);
    if (r.success) {
      open = false;
      await load();
    } else {
      error = r.msg || $tt('failed');
    }
    busy = false;
  }

  async function issue(row, force) {
    await startAndFollow(() =>
      force ? api.cert.renew(row.id, true) : api.cert.apply(row.id, false)
    );
  }

  async function drop(row) {
    if (!confirm($tt('confirmDel'))) return;
    await api.cert.remove(row.id);
    load();
  }

  async function runCron() {
    await startAndFollow(() => api.cert.cron(), $tt('liveLog') + ' · acme.sh --cron');
  }

  function when(ms) {
    const n = Number(ms);
    if (!n) return '—';
    return new Date(n).toLocaleString();
  }

  function jobLabel() {
    if (!busy) return $tt('lastLog');
    const row = rows.find((item) => item.id === jobCertId);
    const name = row && row.domain ? row.domain : jobCertId;
    if (jobKind === 'cron') return $tt('liveLog') + ' · acme.sh --cron';
    if (name) return $tt('liveLog') + ' · ' + name;
    return $tt('liveLog');
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('cert')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  {$tt('compatHint')}
</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

<Card.Root class="mb-4">
  <Card.Header>
    <Card.Title>acme.sh</Card.Title>
    <Card.Description>
      {#if acme.found}
        {acme.path} · ssl: {acme.sslDir || '/etc/nginx/ssl'}
      {:else}
        {$tt('acmeMissing')}
      {/if}
    </Card.Description>
  </Card.Header>
  <Card.Content class="flex flex-wrap items-center gap-2">
    <Badge variant={acme.hasCf ? 'default' : 'outline'}>CF {acme.hasCf ? 'ok' : '—'}</Badge>
    <Badge variant={acme.hasGcore ? 'default' : 'outline'}>Gcore {acme.hasGcore ? 'ok' : '—'}</Badge>
    <Button variant="outline" onclick={runCron} disabled={busy || !acme.found}>acme.sh --cron</Button>
    {#if !acme.found && canInstall}
      <Button variant="outline" disabled={busy} onclick={async () => { busy = true; log = $tt('installing'); const r = await api.install.acme(); log = (r.obj || {}).output || r.msg || ''; busy = false; load(); }}>{$tt('installAcme')}</Button>
    {/if}
    <Button variant="outline" href="/about">{$tt('settings')}</Button>
    <Button variant="outline" disabled={busy} onclick={async () => { busy = true; const r = await api.cert.scan(); log = JSON.stringify((r.obj || {}).adopted || {}, null, 2); busy = false; load(); }}>{$tt('scanCerts')}</Button>
    <Button onclick={startNew}>{$tt('add')}</Button>
  </Card.Content>
</Card.Root>

<Card.Root class="mb-4 overflow-hidden py-0">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>{$tt('domain')}</Table.Head>
        <Table.Head>DNS</Table.Head>
        <Table.Head>{$tt('source')}</Table.Head>
        <Table.Head>file</Table.Head>
        <Table.Head>{$tt('uptime')}</Table.Head>
        <Table.Head></Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each rows as row}
        <Table.Row>
          <Table.Cell class="font-medium">{row.domain}</Table.Cell>
          <Table.Cell><Badge variant="secondary">{row.kind || 'dns_cf'}</Badge></Table.Cell>
          <Table.Cell><Badge variant="outline">{row.source || (row.imported ? $tt('imported') : '—')}</Badge></Table.Cell>
          <Table.Cell><code class="text-muted-foreground font-mono text-xs">{row.cer || row.pem || '—'}</code></Table.Cell>
          <Table.Cell class="text-muted-foreground text-xs">{when(row.makeTime)}</Table.Cell>
          <Table.Cell class="text-right whitespace-nowrap">
            <Button variant="outline" size="sm" disabled={busy} onclick={() => issue(row, false)}>{$tt('issue')}</Button>
            <Button variant="outline" size="sm" disabled={busy} onclick={() => issue(row, true)}>{$tt('forceIssue')}</Button>
            <Button variant="ghost" size="sm" onclick={() => startEdit(row)}>{$tt('edit')}</Button>
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

{#if sslFiles.length}
  <p class="text-muted-foreground mb-4 font-mono text-xs">
    ssl: {sslFiles.map((f) => f.name + '.cer').join(' · ')}
  </p>
{/if}

{#if log || busy}
  <Card.Root class="mb-4">
    <Card.Header>
      <Card.Title class="flex items-center gap-2">
        {jobLabel()}
        {#if busy}
          <Badge>{ $tt('issuing') }</Badge>
        {/if}
      </Card.Title>
    </Card.Header>
    <Card.Content><pre class="code" bind:this={logEl}>{log}</pre></Card.Content>
  </Card.Root>
{/if}

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{editing?.id ? $tt('edit') : $tt('add')}</Dialog.Title>
    </Dialog.Header>
    {#if editing}
      <div class="grid gap-3">
        <div class="grid gap-1.5">
          <Label>{$tt('domain')}</Label>
          <Input bind:value={editing.domain} placeholder="www.example.com 或 *.cdn.example.com" />
        </div>
        <div class="grid gap-1.5">
          <Label>DNS plugin</Label>
          <NativeSelect.Root bind:value={editing.kind} class="w-full">
            <NativeSelect.Option value="dns_cf">{$tt('dnsCf')} (dns_cf)</NativeSelect.Option>
            <NativeSelect.Option value="dns_gcore">{$tt('dnsGcore')} (dns_gcore)</NativeSelect.Option>
            <NativeSelect.Option value="import">{$tt('import')}</NativeSelect.Option>
          </NativeSelect.Root>
        </div>
        <div class="grid gap-1.5">
          <Label>{$tt('remark')}</Label>
          <Input bind:value={editing.descr} />
        </div>
        {#if editing.kind === 'import'}
          <div class="grid gap-1.5">
            <Label>{$tt('pemFile')}</Label>
            <Textarea bind:value={editing.pemContent} rows={6} placeholder="-----BEGIN CERTIFICATE-----" />
          </div>
          <div class="grid gap-1.5">
            <Label>{$tt('keyFile')}</Label>
            <Textarea bind:value={editing.keyContent} rows={4} placeholder="-----BEGIN PRIVATE KEY-----" />
          </div>
        {/if}
      </div>
    {/if}
    <Dialog.Footer>
      <Button variant="outline" onclick={() => (open = false)}>{$tt('cancel')}</Button>
      <Button onclick={save} disabled={busy}>{$tt('save')}</Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
