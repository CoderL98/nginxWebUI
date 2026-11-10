<script>
  import Crud from '$lib/Crud.svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';

  const columns = [
    { key: 'name', label: 'name2' },
    { key: 'descr', label: 'remark' },
    { key: 'path', label: 'root' }
  ];
  const fields = [
    { key: 'name', label: 'name2' },
    { key: 'pass', label: 'pass' },
    { key: 'descr', label: 'remark' }
  ];
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('password')}</h1>
<p class="text-muted-foreground mb-4 text-sm">HTTP Basic Auth。Linux 写入 $apr1$ 哈希，供 nginx auth_basic_user_file 使用。</p>
<Crud {columns} {fields} load={() => api.password.list()} save={(row) => api.password.save(row)} remove={(id) => api.password.remove(id)} blank={{ name: '', pass: '', descr: '' }} />
