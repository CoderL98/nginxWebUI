<script>
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { api, onUnauthorized } from '$lib/api';
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

  const menu = [
    { href: '/', key: 'dash' },
    { href: '/server', key: 'site' },
    { href: '/www', key: 'www' },
    { href: '/cert', key: 'cert' },
    { href: '/conf', key: 'conf' },
    { href: '/log', key: 'log' },
    { href: '/password', key: 'password' },
    { href: '/deny-allow', key: 'denyAllow' },
    { href: '/bak', key: 'bak' },
    { href: '/about', key: 'settings' }
  ];

  async function loadInfo() {
    try {
      const r = await api.login.info();
      const info = r.obj || {};
      signedIn = !!info.signedIn;
      needInit = !!info.needInit;
      adminName = info.name || '';
      version = info.version || '';
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

  let path = $derived($page.url.pathname);
  let active = $derived(path === '/' ? '/' : '/' + (path.split('/')[1] || ''));
</script>

{#if ready && !signedIn}
  <Login {needInit} />
{:else}
  <div class="bg-background min-h-svh md:grid md:grid-cols-[13rem_1fr]">
    <header class="bg-background sticky top-0 z-10 flex h-12 items-center gap-2 border-b px-3 md:col-span-2">
      <Button variant="ghost" size="sm" class="md:hidden" onclick={() => (menuOpen = !menuOpen)}>☰</Button>
      <a href="/" data-sveltekit-reload class="text-sm font-semibold">nginxWebUI</a>
      {#if version}<span class="text-muted-foreground text-[11px]">v{version}</span>{/if}
      <div class="flex-1"></div>
      <Button variant="ghost" size="sm" onclick={toggleLang}>{$tt('lang')}</Button>
      <span class="text-muted-foreground hidden text-xs sm:inline">{adminName}</span>
      <Button variant="outline" size="sm" onclick={signOut}>{$tt('logout')}</Button>
    </header>

    <aside class="border-b md:sticky md:top-12 md:h-[calc(100svh-3rem)] md:border-r md:border-b-0 {menuOpen ? 'block' : 'hidden'} md:block">
      <nav class="flex flex-col gap-0.5 p-2">
        {#each menu as item}
          <a
            href={item.href}
            data-sveltekit-reload
            class="hover:bg-muted rounded-md px-3 py-2 text-sm {active === item.href ? 'bg-muted font-medium' : 'text-muted-foreground'}"
            onclick={() => (menuOpen = false)}
          >
            {$tt(item.key)}
          </a>
        {/each}
        <div class="text-muted-foreground mt-3 px-3 text-[11px] uppercase">advanced</div>
        <p class="text-muted-foreground px-3 pb-1 text-[11px] leading-snug">{$tt('siteVsAdvanced')}</p>
        <a href="/upstream" data-sveltekit-reload class="hover:bg-muted rounded-md px-3 py-1.5 text-sm {active === '/upstream' ? 'bg-muted font-medium' : 'text-muted-foreground'}">{$tt('upstream')}</a>
        <a href="/stream" data-sveltekit-reload class="hover:bg-muted rounded-md px-3 py-1.5 text-sm {active === '/stream' ? 'bg-muted font-medium' : 'text-muted-foreground'}">{$tt('stream')}</a>
        <a href="/http" data-sveltekit-reload class="hover:bg-muted rounded-md px-3 py-1.5 text-sm {active === '/http' ? 'bg-muted font-medium' : 'text-muted-foreground'}">{$tt('http')}</a>
        <a href="/basic" data-sveltekit-reload class="hover:bg-muted rounded-md px-3 py-1.5 text-sm {active === '/basic' ? 'bg-muted font-medium' : 'text-muted-foreground'}">{$tt('basic')}</a>
      </nav>
    </aside>

    <main class="min-w-0 p-4 md:p-6">
      {@render children?.()}
    </main>
  </div>
{/if}
