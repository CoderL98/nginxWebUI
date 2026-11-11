<script>
  import { api } from '$lib/api';
  import { tt, toggleLang } from '$lib/i18n';
  import { Button } from '$lib/components/ui/button/index.js';
  import * as Card from '$lib/components/ui/card/index.js';
  import { Input } from '$lib/components/ui/input/index.js';
  import { Label } from '$lib/components/ui/label/index.js';
  import * as Alert from '$lib/components/ui/alert/index.js';

  let { needInit = false } = $props();

  let name = $state('');
  let pass = $state('');
  let code = $state('');
  let initToken = $state('');
  let message = $state('');
  let busy = $state(false);
  let nonce = $state(Date.now());

  function failText(why) {
    if (why === 'codeError') return $tt('needCode');
    if (why === 'adminError') return $tt('wrong');
    if (why === 'nameTooShort') return $tt('needName');
    if (why === 'initTokenError') return $tt('initTokenError');
    if (why === 'tooManyAttempts') return $tt('tooManyAttempts');
    if (why === 'adminExists') return $tt('adminExists');
    return why || $tt('failed');
  }

  async function signIn() {
    message = '';
    if (!name.trim()) {
      message = $tt('needName');
      return;
    }
    if (!pass) {
      message = $tt('needPass');
      return;
    }
    if (!code.trim()) {
      message = $tt('needCode');
      return;
    }
    if (needInit && !initToken.trim()) {
      message = $tt('needInitToken');
      return;
    }
    busy = true;
    try {
      const r = needInit
        ? await api.login.addAdmin(name.trim(), pass, code.trim(), initToken.trim())
        : await api.login.signIn(name.trim(), pass, code.trim());
      if (r.success) {
        location.reload();
      } else {
        message = failText(r.msg);
        nonce = Date.now();
        code = '';
      }
    } catch {
      message = $tt('failed');
    }
    busy = false;
  }
</script>

<div class="bg-muted/40 flex min-h-svh items-center justify-center p-6">
  <Card.Root class="w-full max-w-sm">
    <Card.Header>
      <div class="flex items-start justify-between gap-3">
        <div>
          <Card.Title>nginxWebUI</Card.Title>
          <Card.Description>{$tt('login')}</Card.Description>
        </div>
        <Button variant="ghost" size="sm" onclick={toggleLang}>{$tt('lang')}</Button>
      </div>
    </Card.Header>
    <Card.Content class="grid gap-3">
      {#if message}
        <Alert.Root variant="destructive"><Alert.Title>{message}</Alert.Title></Alert.Root>
      {/if}
      {#if needInit}
        <Alert.Root><Alert.Title>{$tt('firstRun')}</Alert.Title></Alert.Root>
        <p class="text-muted-foreground text-xs">{$tt('initTokenHint')}</p>
      {/if}
      <div class="grid gap-1.5">
        <Label>{$tt('name')}</Label>
        <Input bind:value={name} autocomplete="username" />
      </div>
      <div class="grid gap-1.5">
        <Label>{$tt('pass')}</Label>
        <Input
          type="password"
          bind:value={pass}
          autocomplete="current-password"
          onkeydown={(e) => e.key === 'Enter' && signIn()}
        />
      </div>
      {#if needInit}
        <div class="grid gap-1.5">
          <Label>{$tt('initToken')}</Label>
          <Input
            bind:value={initToken}
            autocomplete="off"
            onkeydown={(e) => e.key === 'Enter' && signIn()}
          />
        </div>
      {/if}
      <div class="grid gap-1.5">
        <Label>{$tt('code')}</Label>
        <div class="flex items-center gap-2">
          <Input
            bind:value={code}
            maxlength={8}
            class="flex-1"
            autocomplete="off"
            onkeydown={(e) => e.key === 'Enter' && signIn()}
          />
          <button type="button" onclick={() => (nonce = Date.now())} class="shrink-0">
            <img
              class="border-input h-10 w-[168px] rounded-md border bg-muted"
              src={'/adminPage/login/getCode?t=' + nonce}
              alt={$tt('code')}
            />
          </button>
        </div>
      </div>
    </Card.Content>
    <Card.Footer>
      <Button class="w-full" onclick={signIn} disabled={busy}>
        {needInit ? $tt('addAdmin') : $tt('signIn')}
      </Button>
    </Card.Footer>
  </Card.Root>
</div>
