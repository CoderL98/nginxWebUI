<script>
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { onDestroy } from 'svelte';
  import { api, onUnauthorized, getCurrentNode, setCurrentNode } from '$lib/api';
  import { tt, toggleLang } from '$lib/i18n';
  import Login from '$lib/Login.svelte';
  import { Button } from '$lib/components/ui/button/index.js';

  let { children } = $props();

  let ready = $state(false);
  let signedIn = $state(false);
  let needInit = $state(false);
  let adminName = $state('');
  let version = $state('');
  let menuOpen = $state(false);
  let nodes = $state([{ id: 'local', name: 'local' }]);
  let nodeId = $state(getCurrentNode());

  // scope: 'node' = follows the selected device; 'panel' = always the panel
  // host (settings, admins, backups); 'local' = only works on the panel host
  const menu = [
    { href: '/', key: 'dash', scope: 'node' },
    { href: '/server', key: 'site', scope: 'node' },
    { href: '/www', key: 'www', scope: 'local' },
    { href: '/cert', key: 'cert', scope: 'node' },
    { href: '/conf', key: 'conf', scope: 'node' },
    { href: '/log', key: 'log', scope: 'node' },
    { href: '/password', key: 'password', scope: 'local' },
    { href: '/admins', key: 'admins', scope: 'panel' },
    { href: '/nodes', key: 'nodes', scope: 'panel' },
    { href: '/deny-allow', key: 'denyAllow', scope: 'local' },
    { href: '/bak', key: 'bak', scope: 'panel' },
    { href: '/about', key: 'settings', scope: 'panel' }
  ];
  const advanced = [
    { href: '/upstream', key: 'upstream', scope: 'local' },
    { href: '/stream', key: 'stream', scope: 'local' },
    { href: '/http', key: 'http', scope: 'local' },
    { href: '/basic', key: 'basic', scope: 'local' }
  ];
  const scopes = Object.fromEntries([...menu, ...advanced].map((m) => [m.href, m.scope]));

  let timer = null;

  function statusText(status) {
    if (status === 'online') return $tt('nodeOnline');
    if (status === 'pending') return $tt('nodePending');
    if (status === 'revoked') return $tt('nodeRevoked');
    return $tt('nodeOffline');
  }

  async function loadNodes() {
    try {
      const nr = await api.node.list();
      const list = Array.isArray(nr.obj) ? nr.obj : [];
      nodes = list.length ? list : [{ id: 'local', name: 'local', kind: 'local' }];
      const selected = nodes.find((n) => n.id === nodeId);
      // the selected device was deleted or revoked: fall back to this host so
      // a stale id is not sent with every request
      if (!selected || selected.status === 'revoked') {
        if (nodeId !== 'local') {
          nodeId = 'local';
          setCurrentNode('local');
          location.reload();
        }
      }
    } catch {
      /* keep the previous list */
    }
  }

  async function loadInfo() {
    try {
      const r = await api.login.info();
      const info = r.obj || {};
      signedIn = !!info.signedIn;
      needInit = !!info.needInit;
      adminName = info.name || '';
      version = info.version || '';
      if (signedIn) {
        await loadNodes();
        if (!timer) timer = setInterval(loadNodes, 15000);
      }
    } catch {
      signedIn = false;
    }
    ready = true;
  }

  async function signOut() {
    try {
      await api.login.logout();
    } catch {
      /* session gone */
    }
    signedIn = false;
    location.pathname = '/';
  }

  onMount(() => {
    loadInfo();
    onUnauthorized(() => {
      signedIn = false;
      ready = true;
    });
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  function switchNode(id) {
    nodeId = id;
    setCurrentNode(id);
    location.reload();
  }

  let path = $derived($page.url.pathname);
  let active = $derived(path === '/' ? '/' : '/' + (path.split('/')[1] || ''));
  let remote = $derived(nodeId !== 'local');
  let current = $derived(nodes.find((n) => n.id === nodeId) || null);
  let scope = $derived(scopes[active] || 'node');
</script>

{#if ready && !signedIn}
  <Login {needInit} />
{:else}
  <div class="bg-background min-h-svh md:grid md:grid-cols-[13rem_1fr]">
    <header class="bg-background sticky top-0 z-10 flex h-12 items-center gap-2 border-b px-3 md:col-span-2">
      <Button variant="ghost" size="sm" class="md:hidden" aria-label={$tt('menu')} onclick={() => (menuOpen = !menuOpen)}>☰</Button>
      <a href="/" class="text-sm font-semibold">nginxWebUI</a>
      {#if version}<span class="text-muted-foreground text-[11px]">v{version}</span>{/if}
      <div class="flex-1"></div>
      <label class="text-muted-foreground flex items-center gap-1 text-xs">
        <span class="hidden sm:inline">{$tt('currentNode')}</span>
        <select
          class="border-input bg-background h-8 max-w-[9.5rem] rounded-md border px-2 text-xs sm:max-w-none {remote && current && current.status !== 'online' ? 'border-destructive text-destructive' : ''}"
          aria-label={$tt('currentNode')}
          value={nodeId}
          onchange={(e) => switchNode(e.currentTarget.value)}
        >
          {#each nodes as n}
            <option value={n.id}>{n.kind === 'local' ? n.name || 'local' : `${n.name || n.id} (${statusText(n.status)})`}</option>
          {/each}
        </select>
      </label>
      <Button variant="ghost" size="sm" onclick={toggleLang}>{$tt('lang')}</Button>
      <span class="text-muted-foreground hidden text-xs sm:inline">{adminName}</span>
      <Button variant="outline" size="sm" onclick={signOut}>{$tt('logout')}</Button>
    </header>

    <aside class="border-b md:sticky md:top-12 md:h-[calc(100svh-3rem)] md:border-r md:border-b-0 {menuOpen ? 'block' : 'hidden'} md:block">
      <nav class="flex flex-col gap-0.5 p-2">
        {#each menu as item}
          <a
            href={item.href}
            class="hover:bg-muted flex items-center justify-between gap-2 rounded-md px-3 py-2 text-sm {active === item.href ? 'bg-muted font-medium' : 'text-muted-foreground'} {remote && item.scope === 'local' ? 'opacity-50' : ''}"
            onclick={() => (menuOpen = false)}
          >
            <span>{$tt(item.key)}</span>
            {#if remote && item.scope !== 'node'}
              <span class="text-muted-foreground rounded border px-1 text-[10px]">{$tt('panelOnlyTag')}</span>
            {/if}
          </a>
        {/each}
        <div class="text-muted-foreground mt-3 px-3 text-[11px] uppercase">advanced</div>
        <p class="text-muted-foreground px-3 pb-1 text-[11px] leading-snug">{$tt('siteVsAdvanced')}</p>
        {#each advanced as item}
          <a
            href={item.href}
            class="hover:bg-muted flex items-center justify-between gap-2 rounded-md px-3 py-1.5 text-sm {active === item.href ? 'bg-muted font-medium' : 'text-muted-foreground'} {remote ? 'opacity-50' : ''}"
            onclick={() => (menuOpen = false)}
          >
            <span>{$tt(item.key)}</span>
            {#if remote}
              <span class="text-muted-foreground rounded border px-1 text-[10px]">{$tt('panelOnlyTag')}</span>
            {/if}
          </a>
        {/each}
      </nav>
    </aside>

    <main class="min-w-0 p-4 md:p-6">
      {#if remote && current}
        {#if scope === 'local'}
          <div role="alert" class="border-destructive/50 text-destructive mb-4 flex flex-wrap items-center gap-2 rounded-md border px-3 py-2 text-sm">
            <span>{$tt('remoteUnsupported')}</span>
            <Button variant="outline" size="sm" onclick={() => switchNode('local')}>{$tt('switchToLocal')}</Button>
          </div>
        {:else if scope === 'panel'}
          <div role="status" class="text-muted-foreground mb-4 rounded-md border px-3 py-2 text-sm">{$tt('remotePanelOnly')}</div>
        {:else if current.status !== 'online'}
          <div role="alert" class="border-destructive/50 text-destructive mb-4 rounded-md border px-3 py-2 text-sm">
            {current.status === 'pending' ? $tt('pendingBanner') : $tt('offlineBanner')}
            <span class="font-medium">{current.name}</span>
          </div>
        {:else}
          <div role="status" class="text-muted-foreground mb-4 text-xs">{$tt('remoteDevice')}: <span class="font-medium">{current.name}</span>{current.hostname ? ` · ${current.hostname}` : ''}</div>
        {/if}
      {/if}
      {@render children?.()}
    </main>
  </div>
{/if}
