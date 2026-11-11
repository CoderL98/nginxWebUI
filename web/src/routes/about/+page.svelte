<script>
  import { onMount } from 'svelte';
  import { api, isRemote } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import { flagClass, flagOn } from '$lib/flag.js';
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
    reloadCmd: 'sudo -n systemctl reload nginx',
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
    autoInstall: false
  });
  let installLog = $state('');
  let secretsSet = $state({ CF_Token: false, CF_Account_ID: false, GCORE_Key: false });
  let remote = $state(false);
  let oldPass = $state('');
  let newPass = $state('');
  let newPass2 = $state('');

  onMount(async () => {
    remote = isRemote();
    const i = await api.login.info();
    info = i.obj || {};
    const m = await api.monitor();
    monitor = m.obj || {};
    acme = (m.obj || {}).acme || {};
    const c = await api.conf.show();
    const conf = c.obj || {};
    if (remote) {
      form = {
        ...form,
        nginxPath: conf.nginxPath || form.nginxPath,
        nginxDir: conf.nginxDir || form.nginxDir,
        nginxExe: conf.nginxExe || 'nginx',
        reloadCmd: conf.reloadCmd || form.reloadCmd,
        sitesAvailable: conf.sitesAvailable || form.sitesAvailable,
        sitesEnabled: conf.sitesEnabled || form.sitesEnabled,
        sslDir: conf.sslDir || form.sslDir,
        sslParams: conf.sslParams || form.sslParams,
        webRoot: conf.webRoot || form.webRoot,
        acmeSh: acme.path || form.acmeSh,
        acmeHome: acme.home || form.acmeHome,
        acmeEmail: acme.email || '',
        autoInstall: flagOn(monitor.autoInstall, false)
      };
      return;
    }
    const getSecret = async (key) => {
      const obj = (await api.setting.get(key)).obj || {};
      return { set: !!obj.set, value: obj.set ? '' : (obj.value || '') };
    };
    const cf = await getSecret('CF_Token');
    const account = await getSecret('CF_Account_ID');
    const gcore = await getSecret('GCORE_Key');
    secretsSet = { CF_Token: cf.set, CF_Account_ID: account.set, GCORE_Key: gcore.set };
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
      CF_Token: '',
      CF_Account_ID: '',
      GCORE_Key: '',
      autoInstall: flagOn(monitor.autoInstall, false)
    };
  });

  async function install(kind) {
    if (!confirm($tt('confirmInstall'))) return;
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
    if (remote) {
      const r = await api.install.auto(flagOn(form.autoInstall, false));
      message = r.success ? $tt('saved') : ($tt(r.msg) !== r.msg ? $tt(r.msg) : (r.msg || $tt('failed')));
      busy = false;
      return;
    }
    const snap = $state.snapshot(form);
    const payload = { ...snap, autoInstall: flagOn(snap.autoInstall, false) ? 'true' : 'false' };
    if (!String(payload.CF_Token || '').trim()) delete payload.CF_Token;
    if (!String(payload.CF_Account_ID || '').trim()) delete payload.CF_Account_ID;
    if (!String(payload.GCORE_Key || '').trim()) delete payload.GCORE_Key;
    const r = await api.conf.saveCmd(payload);
    await api.install.auto(flagOn(form.autoInstall, false));
    message = r.success ? $tt('saved') : ($tt(r.msg) !== r.msg ? $tt(r.msg) : (r.msg || $tt('failed')));
    busy = false;
  }

  async function changePassword() {
    message = '';
    if (newPass.length < 6) {
      message = $tt('passTooShort');
      return;
    }
    if (newPass !== newPass2) {
      message = $tt('passMismatch');
      return;
    }
    busy = true;
    const r = await api.login.changePass(oldPass, newPass);
    message = r.success ? $tt('passChanged') : (r.msg === 'oldPassError' ? $tt('oldPassError') : r.msg || $tt('failed'));
    if (r.success) {
      oldPass = '';
      newPass = '';
      newPass2 = '';
    }
    busy = false;
  }
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('settings')}</h1>
<p class="text-muted-foreground mb-4 text-sm">
  {remote ? $tt('remoteInstallHint') : $tt('settingsIntro')}
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
        <Label>{$tt('acmeHome')}</Label>
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
    <Card.Title>{$tt('nginxPaths')}</Card.Title>
    <Card.Description>{$tt('compatHint')}</Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>{$tt('nginxExe')}</Label>
      <Input bind:value={form.nginxExe} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('reloadCmd')}</Label>
      <select class="border-input bg-background h-9 rounded-md border px-2 text-sm" bind:value={form.reloadCmd}>
        <option value="sudo -n systemctl reload nginx">sudo -n systemctl reload nginx</option>
        <option value="systemctl reload nginx">systemctl reload nginx</option>
        <option value="nginx -s reload">nginx -s reload</option>
        <option value="/usr/sbin/nginx -s reload">/usr/sbin/nginx -s reload</option>
        <option value="/usr/local/sbin/nginxwebui-priv reload">nginxwebui-priv reload</option>
      </select>
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
      <Label>{$tt('sitesAvailable')}</Label>
      <Input bind:value={form.sitesAvailable} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('sitesEnabled')}</Label>
      <Input bind:value={form.sitesEnabled} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('sslDir')}</Label>
      <Input bind:value={form.sslDir} />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('sslParams')}</Label>
      <Input bind:value={form.sslParams} />
    </div>
    <div class="grid gap-1.5 sm:col-span-2">
      <Label>{$tt('webRoot')}</Label>
      <Input bind:value={form.webRoot} />
    </div>
    <label class="{flagClass(form.autoInstall)} sm:col-span-2 w-fit">
      <Switch bind:checked={form.autoInstall} />
      {$tt('autoInstall')}
    </label>
    {#if remote}
      <div class="sm:col-span-2">
        <Button onclick={save} disabled={busy}>{$tt('save')}</Button>
      </div>
    {/if}
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

{#if !remote}
<Card.Root class="mt-4">
  <Card.Header>
    <Card.Title>{$tt('dnsCreds')}</Card.Title>
    <Card.Description>
      {$tt('dnsCredsHint')}
    </Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-2">
    <div class="grid gap-1.5">
      <Label>CF_Token {secretsSet.CF_Token ? '(' + $tt('secretSet') + ')' : ''}</Label>
      <Input type="password" bind:value={form.CF_Token} autocomplete="off" placeholder={secretsSet.CF_Token ? $tt('secretKeep') : ''} />
    </div>
    <div class="grid gap-1.5">
      <Label>CF_Account_ID {secretsSet.CF_Account_ID ? '(' + $tt('secretSet') + ')' : ''}</Label>
      <Input bind:value={form.CF_Account_ID} autocomplete="off" placeholder={secretsSet.CF_Account_ID ? $tt('secretKeep') : ''} />
    </div>
    <div class="grid gap-1.5 sm:col-span-2">
      <Label>GCORE_Key {secretsSet.GCORE_Key ? '(' + $tt('secretSet') + ')' : ''}</Label>
      <Input type="password" bind:value={form.GCORE_Key} autocomplete="off" placeholder={secretsSet.GCORE_Key ? $tt('secretKeep') : ''} />
    </div>
  </Card.Content>
  <Card.Footer>
    <Button onclick={save} disabled={busy}>{$tt('save')}</Button>
  </Card.Footer>
</Card.Root>

<Card.Root class="mt-4">
  <Card.Header>
    <Card.Title>{$tt('changePass')}</Card.Title>
    <Card.Description>{$tt('changePassHint')}</Card.Description>
  </Card.Header>
  <Card.Content class="grid gap-3 sm:grid-cols-3">
    <div class="grid gap-1.5">
      <Label>{$tt('oldPass')}</Label>
      <Input type="password" bind:value={oldPass} autocomplete="current-password" />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('newPass')}</Label>
      <Input type="password" bind:value={newPass} autocomplete="new-password" />
    </div>
    <div class="grid gap-1.5">
      <Label>{$tt('newPass2')}</Label>
      <Input type="password" bind:value={newPass2} autocomplete="new-password" />
    </div>
  </Card.Content>
  <Card.Footer>
    <Button onclick={changePassword} disabled={busy}>{$tt('changePass')}</Button>
  </Card.Footer>
</Card.Root>
{/if}
