<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let m = $state({});
  let error = $state('');
  let message = $state('');
  let busy = $state(false);

  async function refresh() {
    try {
      const r = await api.monitor();
      m = r.obj || {};
    } catch {
      error = $tt('failed');
    }
  }

  function mb(kb) {
    const value = Number(kb) || 0;
    if (value > 1048576) return (value / 1048576).toFixed(1) + ' GB';
    return (value / 1024).toFixed(1) + ' MB';
  }

  function elapsed(seconds) {
    const total = Number(seconds) || 0;
    const d = Math.floor(total / 86400);
    const h = Math.floor((total % 86400) / 3600);
    const min = Math.floor((total % 3600) / 60);
    const parts = [];
    if (d) parts.push(d + $tt('days'));
    if (h) parts.push(h + $tt('hours'));
    parts.push(min + $tt('minutes'));
    return parts.join(' ');
  }

  onMount(() => {
    refresh();
    const timer = setInterval(refresh, 30000);
    return () => clearInterval(timer);
  });

  async function check() {
    busy = true;
    message = '';
    try {
      const r = await api.conf.check();
      const out = r.obj || {};
      message = (out.success ? '✓ ' : '✗ ') + (out.output || '');
    } catch {
      message = $tt('failed');
    }
    busy = false;
  }

  async function reload() {
    busy = true;
    message = '';
    try {
      const r = await api.conf.reload();
      const out = r.obj || {};
      message = (out.output || '') + (out.success ? ' ✓' : ' ✗');
      refresh();
    } catch {
      message = $tt('failed');
    }
    busy = false;
  }

  async function signal(action) {
    busy = true;
    message = '';
    try {
      const r = await api.conf.signal(action);
      const out = r.obj || {};
      message = (out.output || '') + (out.success ? ' ✓' : ' ✗');
      refresh();
    } catch {
      message = $tt('failed');
    }
    busy = false;
  }

  async function install(kind) {
    busy = true;
    message = $tt('installing');
    try {
      const r = kind === 'nginx' ? await api.install.nginx() : await api.install.acme();
      message = r.success ? (r.obj || {}).output || $tt('installed') : r.msg || $tt('failed');
      refresh();
    } catch {
      message = $tt('failed');
    }
    busy = false;
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('monitor')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  nginxWebUI {m.version || ''} · {m.os || ''}/{m.arch || ''} · {mb(m.processMem)}
</p>

{#if error}
  <Alert.Root variant="destructive" class="mb-4"><Alert.Title>{error}</Alert.Title></Alert.Root>
{/if}

{#if m.nginxFound === false}
  <Alert.Root class="mb-3">
    <Alert.Title>{$tt('nginxMissing')}</Alert.Title>
    <Alert.Description>
      {#if m.canInstall}
        <Button size="sm" class="mt-2" disabled={busy} onclick={() => install('nginx')}>{$tt('installNginx')}</Button>
      {:else}
        {$tt('installLinuxOnly')}
      {/if}
    </Alert.Description>
  </Alert.Root>
{/if}
{#if m.acmeFound === false}
  <Alert.Root class="mb-3">
    <Alert.Title>{$tt('acmeMissing')}</Alert.Title>
    <Alert.Description>
      {#if m.canInstall}
        <Button size="sm" class="mt-2" disabled={busy} onclick={() => install('acme')}>{$tt('installAcme')}</Button>
      {:else}
        {$tt('installLinuxOnly')}
      {/if}
    </Alert.Description>
  </Alert.Root>
{/if}

<div class="mb-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
  <Card.Root>
    <Card.Header>
      <Card.Description>{$tt('mem')}</Card.Description>
      <Card.Title class="text-2xl tabular-nums">{m.memPercent ?? 0}%</Card.Title>
    </Card.Header>
    <Card.Content>
      <div class="bg-muted h-1 overflow-hidden rounded-full">
        <div class="bg-primary h-full" style="width:{(m.memPercent ?? 0) + '%'}"></div>
      </div>
      <p class="text-muted-foreground mt-2 text-xs">{mb(m.memUsed)} / {mb(m.memTotal)}</p>
    </Card.Content>
  </Card.Root>
  <Card.Root>
    <Card.Header>
      <Card.Description>{$tt('cpu')}</Card.Description>
      <Card.Title class="text-2xl tabular-nums">{m.cpuUsage ?? 0}%</Card.Title>
    </Card.Header>
    <Card.Content>
      <div class="bg-muted h-1 overflow-hidden rounded-full">
        <div class="bg-primary h-full" style="width:{(m.cpuUsage ?? 0) + '%'}"></div>
      </div>
      <p class="text-muted-foreground mt-2 text-xs">{m.cpuCores ?? 0} {$tt('cores')}</p>
    </Card.Content>
  </Card.Root>
  <Card.Root>
    <Card.Header>
      <Card.Description>{$tt('uptime')}</Card.Description>
      <Card.Title class="text-xl">{elapsed(m.uptime)}</Card.Title>
    </Card.Header>
  </Card.Root>
  <Card.Root>
    <Card.Header>
      <Card.Description>{$tt('nginxRunning')}</Card.Description>
      <Card.Title>
        <Badge variant={m.nginxRunning ? 'default' : 'destructive'}>
          {m.nginxRunning ? $tt('running') : $tt('stopped')}
        </Badge>
      </Card.Title>
    </Card.Header>
    <Card.Content>
      <p class="text-muted-foreground break-all font-mono text-xs">{m.nginxPath || ''}</p>
    </Card.Content>
  </Card.Root>
</div>

<Card.Root>
  <Card.Header>
    <Card.Title>{$tt('conf')}</Card.Title>
    <Card.Description>
      {$tt('site')}: {m.sites ?? 0} · {$tt('sslDir')}: {m.sslDir || ''}
    </Card.Description>
  </Card.Header>
  <Card.Content class="flex flex-wrap gap-2">
    <Button variant="outline" onclick={check} disabled={busy}>{$tt('check')}</Button>
    <Button onclick={reload} disabled={busy}>{$tt('reload')}</Button>
    <Button variant="outline" onclick={() => signal('start')} disabled={busy}>{$tt('start')}</Button>
    <Button variant="outline" onclick={() => signal('stop')} disabled={busy}>{$tt('stop')}</Button>
    <Button variant="outline" href="/conf">{$tt('conf')} →</Button>
  </Card.Content>
  {#if message}
    <Card.Footer>
      <pre class="code w-full">{message}</pre>
    </Card.Footer>
  {/if}
</Card.Root>
