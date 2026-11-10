<script>
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Textarea } from '$lib/components/ui/textarea/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import { Switch } from '$lib/components/ui/switch/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';
  import * as NativeSelect from '$lib/components/ui/native-select/index.js';

  function blankLoc(type = 'static') {
    return {
      path: '/',
      type,
      root: '',
      index: 'index.html',
      proxy: '',
      upstream: '',
      returnUrl: '',
      custom: '',
      websocket: false
    };
  }

  function inferKind(s) {
    if (s.kind === 'forward' || s.kind === 'reverse' || s.kind === 'static') return s.kind;
    if (s.mode === 'forward') return 'forward';
    if (s.mode === 'proxy') return 'reverse';
    const locs = s.locations || [];
    if (locs.some((l) => l.type === 'proxy' || l.type === '0' || l.type === 'upstream' || l.type === '2')) {
      return 'reverse';
    }
    if (s.proxy) return 'reverse';
    return 'static';
  }

  let site = $state({
    kind: 'static',
    domain: '',
    listen: '80',
    ssl: true,
    http2: true,
    redirect: true,
    ipv6: false,
    certName: '',
    descr: '',
    enable: true,
    proxy: '',
    websocket: false,
    resolver: '223.5.5.5 8.8.8.8',
    allow: '',
    locations: [blankLoc('static')]
  });
  let preview = $state('');
  let matched = $state('');
  let sslFiles = $state([]);
  let upstreams = $state([]);
  let note = $state('');
  let error = $state('');
  let busy = $state(false);
  let output = $state('');
  let webRoot = $state('/var/www');

  function defaultRoot(domain) {
    const base = (webRoot || '/var/www').replace(/[\\/]+$/, '');
    return domain ? base + '/' + domain : base;
  }

  function normalizeLoc(loc) {
    const typeMap = { '0': 'proxy', '1': 'static', '2': 'upstream', '3': 'custom', '4': 'redirect' };
    const type = typeMap[loc.type] || loc.type || 'static';
    return {
      ...blankLoc(),
      ...loc,
      type,
      websocket: !!Number(loc.websocket)
    };
  }

  onMount(async () => {
    const id = $page.url.searchParams.get('id') || '';
    const u = await api.upstream.list();
    upstreams = Array.isArray(u.obj) ? u.obj : [];
    if (id) {
      const r = await api.site.detail(id);
      const data = r.obj || {};
      webRoot = data.webRoot || webRoot;
      if (data.site) {
        const s = data.site;
        site = {
          ...site,
          ...s,
          kind: inferKind(s),
          ssl: s.ssl !== false && s.ssl !== 0 && s.ssl !== 'false',
          http2: s.http2 !== false && s.http2 !== 0 && s.http2 !== 'false',
          redirect: s.redirect !== false && s.redirect !== 0 && s.redirect !== 'false',
          ipv6: !!Number(s.ipv6),
          websocket: !!Number(s.websocket),
          proxy: s.proxy || '',
          resolver: s.resolver || '223.5.5.5 8.8.8.8',
          allow: s.allow || '',
          locations: (s.locations && s.locations.length ? s.locations : []).map(normalizeLoc)
        };
        if (!site.locations.length) {
          if (site.kind === 'reverse') {
            site.locations = [normalizeLoc({ path: '/', type: 'proxy', proxy: site.proxy, websocket: site.websocket })];
          } else if (site.kind === 'static') {
            site.locations = [normalizeLoc({ path: '/', type: 'static', root: s.root })];
          }
        }
        const rootProxy = site.locations.find((l) => l.path === '/' && l.type === 'proxy');
        if (rootProxy) {
          site.proxy = site.proxy || rootProxy.proxy;
          site.websocket = site.websocket || rootProxy.websocket;
        }
      }
      preview = data.preview || '';
      matched = data.matchedCert || '';
      sslFiles = data.sslFiles || [];
    } else {
      const c = await api.cert.list();
      sslFiles = (c.obj || {}).sslFiles || [];
      const w = await api.www.list();
      webRoot = (w.obj || {}).webRoot || webRoot;
      const kindParam = $page.url.searchParams.get('kind') || 'static';
      if (kindParam === 'reverse' || kindParam === 'forward' || kindParam === 'static') {
        setKind(kindParam);
      }
    }
  });

  async function lookupCert() {
    if (!site.domain) return;
    const r = await api.site.findCert(site.domain);
    matched = (r.obj || {}).certName || '';
    sslFiles = (r.obj || {}).sslFiles || [];
    if (matched && !site.certName) site.certName = matched;
    if (!site.locations[0]?.root && site.locations[0]?.type === 'static') {
      site.locations[0].root = defaultRoot(site.domain);
    }
  }

  async function showPreview() {
    const r = await api.site.preview(payload(false));
    if (r.success) {
      preview = (r.obj || {}).conf || '';
      error = '';
    } else {
      error = r.msg || $tt('failed');
    }
  }

  function setKind(kind) {
    site.kind = kind;
    if (kind === 'static') {
      site.ssl = true;
      site.redirect = true;
      site.http2 = true;
      if (!site.listen || site.listen === '8888') site.listen = '80';
      const extras = site.locations.filter((l) => l.path !== '/');
      site.locations = [blankLoc('static'), ...extras];
    } else if (kind === 'reverse') {
      site.ssl = true;
      site.redirect = true;
      site.http2 = true;
      if (!site.listen || site.listen === '8888') site.listen = '80';
      if (!site.proxy) site.proxy = '8080';
      const extras = site.locations.filter((l) => l.path !== '/');
      site.locations = [
        normalizeLoc({ path: '/', type: 'proxy', proxy: site.proxy, websocket: site.websocket }),
        ...extras
      ];
    } else if (kind === 'forward') {
      site.ssl = false;
      site.redirect = false;
      site.http2 = false;
      if (!site.listen || site.listen === '80') site.listen = '8888';
      if (!site.domain) site.domain = 'forward-' + site.listen;
      if (!site.resolver) site.resolver = '223.5.5.5 8.8.8.8';
    }
  }

  function payload(apply) {
    const s = { ...site, apply };
    if (s.kind === 'reverse') {
      const rest = (s.locations || []).filter((l) => !(l.path === '/' && l.type === 'proxy'));
      s.locations = [
        { path: '/', type: 'proxy', proxy: s.proxy, websocket: s.websocket },
        ...rest
      ];
      s.mode = 'proxy';
    }
    if (s.kind === 'forward') {
      s.ssl = false;
      s.redirect = false;
      s.mode = 'forward';
      s.locations = [];
    }
    if (s.kind === 'static') s.mode = 'static';
    return s;
  }

  async function save(apply) {
    busy = true;
    error = '';
    note = '';
    output = '';
    try {
      const r = await api.site.save(payload(apply));
      if (r.success) {
        note = $tt('saved');
        output = (r.obj || {}).output || '';
        if (!site.id) {
          site.id = (r.obj || {}).id;
          history.replaceState(null, '', '/server/edit?id=' + encodeURIComponent(site.id));
        }
        await showPreview();
      } else {
        error = r.msg || $tt('failed');
      }
    } catch {
      error = $tt('failed');
    }
    busy = false;
  }

  async function drop() {
    if (!site.id || !confirm($tt('confirmDel'))) return;
    await api.site.remove(site.id);
    location.href = '/server';
  }

  function addLoc() {
    const type = site.kind === 'reverse' ? 'proxy' : 'static';
    site.locations = [...site.locations, { ...blankLoc(type), path: '/extra' }];
  }

  function removeLoc(i) {
    site.locations = site.locations.filter((_, idx) => idx !== i);
    if (!site.locations.length) site.locations = [blankLoc()];
  }

  async function uploadZip(i, file) {
    if (!file) return;
    const loc = site.locations[i];
    const dir = loc.root || defaultRoot(site.domain);
    if (!dir) {
      error = $tt('root');
      return;
    }
    busy = true;
    error = '';
    const r = await api.www.upload(dir, file, site.domain);
    if (r.success) {
      loc.root = (r.obj || {}).path || dir;
      note = $tt('saved');
      output = ((r.obj || {}).files || []).slice(0, 20).join('\n');
    } else {
      error = r.msg || $tt('failed');
    }
    busy = false;
  }
</script>

<div class="mb-4 flex items-baseline gap-3">
  <a href="/server" class="text-muted-foreground text-xs">← {$tt('site')}</a>
  <h1 class="text-lg font-semibold">{site.id ? $tt('edit') : $tt('add')}</h1>
</div>

{#if error}<Alert.Root variant="destructive" class="mb-3"><Alert.Title>{error}</Alert.Title></Alert.Root>{/if}
{#if note}<Alert.Root class="mb-3"><Alert.Title>{note}</Alert.Title></Alert.Root>{/if}

<Card.Root class="mb-4">
  <Card.Content class="grid gap-4 pt-6">
    <div class="grid gap-1.5">
      <Label>{$tt('siteKind')}</Label>
      <div class="grid gap-2 sm:grid-cols-3">
        <button
          type="button"
          class="rounded-lg border p-3 text-left text-sm {site.kind === 'static' ? 'border-primary bg-primary/5' : 'border-border'}"
          onclick={() => setKind('static')}
        >
          <div class="font-medium">{$tt('staticSite')}</div>
          <p class="text-muted-foreground mt-1 text-xs">{$tt('staticHint')}</p>
        </button>
        <button
          type="button"
          class="rounded-lg border p-3 text-left text-sm {site.kind === 'reverse' ? 'border-primary bg-primary/5' : 'border-border'}"
          onclick={() => setKind('reverse')}
        >
          <div class="font-medium">{$tt('reverseProxy')}</div>
          <p class="text-muted-foreground mt-1 text-xs">{$tt('reverseHint')}</p>
        </button>
        <button
          type="button"
          class="rounded-lg border p-3 text-left text-sm {site.kind === 'forward' ? 'border-primary bg-primary/5' : 'border-border'}"
          onclick={() => setKind('forward')}
        >
          <div class="font-medium">{$tt('forwardProxy')}</div>
          <p class="text-muted-foreground mt-1 text-xs">{$tt('forwardHint')}</p>
        </button>
      </div>
    </div>

    <div class="grid gap-3 sm:grid-cols-3">
      <div class="grid gap-1.5">
        <Label>{$tt('listenDomain')}</Label>
        <Input
          bind:value={site.domain}
          placeholder={site.kind === 'forward' ? 'forward-8888' : 'www.example.com'}
          onblur={lookupCert}
        />
      </div>
      {#if site.kind === 'forward' || !site.ssl}
      <div class="grid gap-1.5">
        <Label>{site.kind === 'forward' ? $tt('forwardListen') : $tt('listen')}</Label>
        <Input bind:value={site.listen} placeholder={site.kind === 'forward' ? '8888' : '80'} />
      </div>
      {/if}
      {#if site.kind !== 'forward' && site.ssl}
      <div class="grid gap-1.5">
        <Label>{$tt('selectCert')} {matched ? `(${$tt('matchedCert')}: ${matched})` : ''}</Label>
        <NativeSelect.Root bind:value={site.certName} class="w-full">
          <NativeSelect.Option value="">auto</NativeSelect.Option>
          {#each sslFiles as file}
            <NativeSelect.Option value={file.name}>{file.name}.cer</NativeSelect.Option>
          {/each}
        </NativeSelect.Root>
      </div>
      {/if}
    </div>

    {#if site.kind === 'reverse'}
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="grid gap-1.5">
          <Label>{$tt('proxyTarget')}</Label>
          <Input bind:value={site.proxy} placeholder="8080 或 http://127.0.0.1:8080" />
        </div>
        <label class="flex items-center gap-2 self-end text-sm"><Switch bind:checked={site.websocket} /> {$tt('websocket')}</label>
      </div>
    {/if}

    {#if site.kind === 'forward'}
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="grid gap-1.5">
          <Label>{$tt('forwardResolver')}</Label>
          <Input bind:value={site.resolver} placeholder="223.5.5.5 8.8.8.8" />
        </div>
        <div class="grid gap-1.5">
          <Label>{$tt('forwardAllow')}</Label>
          <Textarea bind:value={site.allow} rows={3} placeholder="10.0.0.0/8" />
          <p class="text-muted-foreground text-xs">{$tt('forwardAllowHint')}</p>
        </div>
      </div>
    {/if}

    {#if site.kind !== 'forward'}
    <div class="flex flex-wrap gap-5">
      <label class="flex items-center gap-2 text-sm"><Switch bind:checked={site.ssl} /> {$tt('enableSsl')}</label>
      <label class="flex items-center gap-2 text-sm"><Switch bind:checked={site.http2} /> {$tt('http2')}</label>
      <label class="flex items-center gap-2 text-sm"><Switch bind:checked={site.redirect} /> {$tt('redirectHttps')}</label>
      <label class="flex items-center gap-2 text-sm"><Switch bind:checked={site.ipv6} /> {$tt('ipv6')}</label>
    </div>
    {:else}
    <div class="flex flex-wrap gap-5">
      <label class="flex items-center gap-2 text-sm"><Switch bind:checked={site.ipv6} /> {$tt('ipv6')}</label>
    </div>
    {/if}

    <div class="grid gap-1.5">
      <Label>{$tt('remark')}</Label>
      <Input bind:value={site.descr} />
    </div>
  </Card.Content>
</Card.Root>

{#if site.kind !== 'forward'}
<Card.Root class="mb-4">
  <Card.Header class="flex-row items-center justify-between">
    <Card.Title>{site.kind === 'reverse' ? $tt('extraLocation') : $tt('location')}</Card.Title>
    <Button size="sm" variant="outline" onclick={addLoc}>{$tt('addLocation')}</Button>
  </Card.Header>
  <Card.Content class="space-y-4">
    {#each site.locations as loc, i}
      {#if !(site.kind === 'reverse' && loc.path === '/' && loc.type === 'proxy')}
      <div class="rounded-lg border p-3">
        <div class="mb-3 grid gap-3 sm:grid-cols-3">
          <div class="grid gap-1.5">
            <Label>{$tt('path')}</Label>
            <Input bind:value={loc.path} placeholder="/" />
          </div>
          <div class="grid gap-1.5">
            <Label>{$tt('type')}</Label>
            <NativeSelect.Root bind:value={loc.type} class="w-full">
              <NativeSelect.Option value="proxy">{$tt('locProxy')}</NativeSelect.Option>
              <NativeSelect.Option value="static">{$tt('locStatic')}</NativeSelect.Option>
              <NativeSelect.Option value="upstream">{$tt('locUpstream')}</NativeSelect.Option>
              <NativeSelect.Option value="redirect">{$tt('locRedirect')}</NativeSelect.Option>
              <NativeSelect.Option value="custom">{$tt('locCustom')}</NativeSelect.Option>
            </NativeSelect.Root>
          </div>
          <div class="flex items-end justify-end">
            <Button variant="ghost" size="sm" onclick={() => removeLoc(i)}>{$tt('del')}</Button>
          </div>
        </div>

        {#if loc.type === 'proxy'}
          <div class="grid gap-3 sm:grid-cols-2">
            <div class="grid gap-1.5">
              <Label>{$tt('proxyTarget')}</Label>
              <Input bind:value={loc.proxy} placeholder="8080 或 http://127.0.0.1:8080" />
            </div>
            <label class="flex items-center gap-2 self-end text-sm"><Switch bind:checked={loc.websocket} /> {$tt('websocket')}</label>
          </div>
        {:else if loc.type === 'static'}
          <div class="grid gap-3 sm:grid-cols-2">
            <div class="grid gap-1.5">
              <Label>{$tt('root')}</Label>
              <Input bind:value={loc.root} placeholder={defaultRoot(site.domain || 'domain')} />
            </div>
            <div class="grid gap-1.5">
              <Label>{$tt('index')}</Label>
              <Input bind:value={loc.index} placeholder="index.html" />
            </div>
          </div>
          <div class="mt-3 flex items-center gap-2">
            <Input
              type="file"
              accept=".zip,application/zip"
              class="max-w-xs"
              onchange={(e) => uploadZip(i, e.currentTarget.files?.[0])}
            />
            <span class="text-muted-foreground text-xs">{$tt('zipToRoot')}</span>
          </div>
        {:else if loc.type === 'upstream'}
          <div class="grid gap-3 sm:grid-cols-2">
            <div class="grid gap-1.5">
              <Label>{$tt('upstream')}</Label>
              <NativeSelect.Root bind:value={loc.upstream} class="w-full">
                <NativeSelect.Option value="">—</NativeSelect.Option>
                {#each upstreams as up}
                  <NativeSelect.Option value={up.name}>{up.name}</NativeSelect.Option>
                {/each}
              </NativeSelect.Root>
            </div>
            <label class="flex items-center gap-2 self-end text-sm"><Switch bind:checked={loc.websocket} /> {$tt('websocket')}</label>
          </div>
        {:else if loc.type === 'redirect'}
          <div class="grid gap-1.5">
            <Label>{$tt('returnUrl')}</Label>
            <Input bind:value={loc.returnUrl} placeholder="https://example.com$request_uri" />
          </div>
        {:else}
          <div class="grid gap-1.5">
            <Label>{$tt('customBody')}</Label>
            <Textarea bind:value={loc.custom} rows={5} placeholder="try_files $uri $uri/ /index.html;" />
          </div>
        {/if}
      </div>
      {/if}
    {/each}
  </Card.Content>
</Card.Root>
{/if}

<div class="mb-4 flex flex-wrap gap-2">
  <Button variant="outline" onclick={showPreview} disabled={busy}>{$tt('preview')}</Button>
  <Button variant="outline" onclick={() => save(false)} disabled={busy}>{$tt('save')}</Button>
  <Button onclick={() => save(true)} disabled={busy}>{$tt('applySite')}</Button>
  {#if site.id}<Button variant="destructive" onclick={drop}>{$tt('del')}</Button>{/if}
</div>

{#if output}<pre class="code mb-4">{output}</pre>{/if}
{#if preview}
  <Card.Root>
    <Card.Header><Card.Title>{site.domain || 'site'}.conf</Card.Title></Card.Header>
    <Card.Content><pre class="code">{preview}</pre></Card.Content>
  </Card.Root>
{/if}
