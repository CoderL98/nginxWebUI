<script>
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let info = $state({});
  let monitor = $state({});
  let acme = $state({});
  let busy = $state(false);
  let message = $state('');
  let form = $state({
    nginxPath: '/etc/nginx/nginx.conf',
    nginxExe: 'nginx',
    reloadCmd: 'sudo systemctl reload nginx',
    sitesAvailable: '/etc/nginx/sites-available',
    sitesEnabled: '/etc/nginx/sites-enabled',
    sslDir: '/etc/nginx/ssl',
    sslParams: '/etc/nginx/ssl-params.conf',
    webRoot: '/var/www',
    acmeSh: '',
    acmeHome: '',
    acmeEmail: '',
    nginxDir: '/etc/nginx',
    CF_Token: '',
    CF_Account_ID: '',
    GCORE_Key: '',
    autoInstall: true
  });
  let installLog = $state('');

  onMount(async () => {
    const i = await api.login.info();
    info = i.obj || {};
    const m = await api.monitor();
    monitor = m.obj || {};
    acme = (m.obj || {}).acme || {};
    const c = await api.conf.show();
    const conf = c.obj || {};
    const get = async (key) => ((await api.setting.get(key)).obj || {}).value || '';
    form = {
      nginxPath: conf.nginxPath || form.nginxPath,
      nginxDir: conf.nginxDir || form.nginxDir,
      nginxExe: conf.nginxExe || 'nginx',
      reloadCmd: conf.reloadCmd || form.reloadCmd,
      sitesAvailable: conf.sitesAvailable || form.sitesAvailable,
      sitesEnabled: conf.sitesEnabled || form.sitesEnabled,
      sslDir: conf.sslDir || form.sslDir,
      sslParams: conf.sslParams || form.sslParams,
      webRoot: conf.webRoot || form.webRoot,
      acmeSh: acme.path || (await get('acmeSh')),
      acmeHome: acme.home || (await get('acmeHome')),
      acmeEmail: acme.email || '',
      CF_Token: await get('CF_Token'),
      CF_Account_ID: await get('CF_Account_ID'),
      GCORE_Key: await get('GCORE_Key'),
      autoInstall: (monitor.autoInstall !== false)
    };
  });

  async function install(kind) {
    busy = true;
    message = $tt('installing');
    installLog = '';
    const r = kind === 'nginx' ? await api.install.nginx() : await api.install.acme();
    message = r.success ? $tt('installed') : r.msg || $tt('failed');
    installLog = (r.obj || {}).output || r.msg || '';
    const m = await api.monitor();
    monitor = m.obj || {};
    acme = (m.obj || {}).acme || {};
    busy = false;
  }

  function mb(kb) {
    return ((Number(kb) || 0) / 1024).toFixed(1) + ' MB';
  }

  async function save() {
    busy = true;
    message = '';
    const r = await api.conf.saveCmd({ ...form, autoInstall: form.autoInstall ? 'true' : 'false' });
    await api.install.auto(form.autoInstall);
    message = r.success ? $tt('saved') : $tt('failed');
    busy = false;
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('settings')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  对齐服务器脚本：sites-available、/etc/nginx/ssl、acme.sh DNS、systemctl reload。凭据只保存在本机 SQLite。
</p>

{#if message}
  <Alert.Root class="mb-3"><Alert.Title>{message}</Alert.Title></Alert.Root>
{/if}

<div class="grid gap-4 lg:grid-cols-2">
  <Card.Root>
    <Card.Header>
      <Card.Title>nginxWebUI {info.version || ''}</Card.Title>
      <Card.Description>SvelteKit + Rust + SQLite</Card.Description>
    </Card.Header>
    <Card.Content class="text-sm">
      <dl class="grid gap-2">
        <div class="flex justify-between gap-4"><dt class="text-muted-foreground">sqlite</dt><dd class="truncate font-mono text-xs">{monitor.dataFile || ''}</dd></div>
        <div class="flex justify-between gap-4"><dt class="text-muted-foreground">{$tt('mem')}</dt><dd>{mb(monitor.processMem)}</dd></div>
      </dl>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title>acme.sh</Card.Title>
      <Card.Description>{acme.found ? acme.path : $tt('acmeMissing')}</Card.Description>
    </Card.Header>
    <Card.Content class="grid gap-3">
      <div class="grid gap-1.5">
        <Label>{$tt('acmePath')}</Label>
        <Input bind:value={form.acmeSh} placeholder="/home/you/.acme.sh/acme.sh" />
      </div>
      <div class="grid gap-1.5">
        <Label>acme home</Label>
        <Input bind:value={form.acmeHome} placeholder="/home/you/.acme.sh" />
      </div>
      <div class="grid gap-1.5">
        <Label>{$tt('acmeEmail')}</Label>
        <Input bind:value={form.acmeEmail} />
      </div>
      {#if !acme.found && monitor.canInstall}
        <Button variant="outline" onclick={() => install('acme')} disabled={busy}>{$tt('installAcme')}</Button>
      {:else if !acme.found}
        <p class="text-muted-foreground text-xs">{$tt('installLinuxOnly')}</p>
      {/if}
    </Card.Content>
  </Card.Root>
</div>

<Card.Root class="mt-4">
  <Card.Header>
    <Card.Title>nginx 路径</Card.Title>
    <Card.Description>{$tt('compatHint')}</Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>{$tt('nginxExe')}</Label>
      <Input bind:value={form.nginxExe} />
    </div>
    <div class="grid gap-1.5">
      <Label>reload</Label>
      <Input bind:value={form.reloadCmd} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('nginxPath')}</Label>
      <Input bind:value={form.nginxPath} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('nginxDir')}</Label>
      <Input bind:value={form.nginxDir} />
    </div>
    <div class="grid gap-1.5">
      <Label>sites-available</Label>
      <Input bind:value={form.sitesAvailable} />
    </div>
    <div class="grid gap-1.5">
      <Label>sites-enabled</Label>
      <Input bind:value={form.sitesEnabled} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('sslDir')}</Label>
      <Input bind:value={form.sslDir} />
    </div>
    <div class="grid gap-1.5">
      <Label>ssl-params.conf</Label>
      <Input bind:value={form.sslParams} />
    </div>
    <div class="grid gap-1.5 sm:col-span-2">
      <Label>/var/www</Label>
      <Input bind:value={form.webRoot} />
    </div>
    <div class="flex items-center gap-2 sm:col-span-2">
      <Switch bind:checked={form.autoInstall} />
      <Label>{$tt('autoInstall')}</Label>
    </div>
    {#if monitor.nginxFound === false}
      <div class="sm:col-span-2">
        {#if monitor.canInstall}
          <Button variant="outline" onclick={() => install('nginx')} disabled={busy}>{$tt('installNginx')}</Button>
        {:else}
          <p class="text-muted-foreground text-xs">{$tt('installLinuxOnly')}</p>
        {/if}
      </div>
    {/if}
  </Card.Content>
</Card.Root>

{#if installLog}
  <pre class="code mt-4">{installLog}</pre>
{/if}

<Card.Root class="mt-4">
  <Card.Header>
    <Card.Title>DNS 凭据</Card.Title>
    <Card.Description>
      传给 acme.sh 的环境变量。也可事先写在 ~/.acme.sh/account.conf（SAVED_CF_Token / SAVED_GCORE_Key）。
    </Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>CF_Token</Label>
      <Input type="password" bind:value={form.CF_Token} autocomplete="off" />
    </div>
    <div class="grid gap-1.5">
      <Label>CF_Account_ID</Label>
      <Input bind:value={form.CF_Account_ID} />
    </div>
    <div class="grid gap-1.5 sm:col-span-2">
      <Label>GCORE_Key</Label>
      <Input type="password" bind:value={form.GCORE_Key} autocomplete="off" />
    </div>
  </Card.Content>
  <Card.Footer>
    <Button onclick={save} disabled={busy}>{$tt('save')}</Button>
  </Card.Footer>
</Card.Root>
