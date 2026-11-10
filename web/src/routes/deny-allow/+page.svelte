<script>
  import Crud from '$lib/Crud.svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';

  const columns = [
    { key: 'name', label: 'name2' },
    { key: 'ip', label: 'value' }
  ];
  const fields = [
    { key: 'name', label: 'name2' },
    { key: 'ip', label: 'value', type: 'area', rows: 8 },
    { key: 'descr', label: 'remark' }
  ];
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('denyAllow')}</h1>
<p class="text-muted-foreground mb-4 text-sm">每行一个地址或网段：192.168.1.0/24, 10.0.0.5, all</p>
<Crud {columns} {fields} load={() => api.denyAllow.list()} save={(row) => api.denyAllow.save(row)} remove={(id) => api.denyAllow.remove(id)} blank={{ name: '', ip: '', descr: '' }} />
