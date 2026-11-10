<script>
  import Crud from '$lib/Crud.svelte';
  import { api } from '$lib/api';
  import { tt } from '$lib/i18n';

  const columns = [
    { key: 'name', label: 'name2' },
    { key: 'value', label: 'value' },
    { key: 'descr', label: 'remark' }
  ];
  const fields = [
    { key: 'name', label: 'name2' },
    { key: 'value', label: 'value', type: 'area', rows: 3 },
    { key: 'descr', label: 'remark' }
  ];
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('basic')}</h1>
<p class="text-muted-foreground mb-4 text-sm">worker_processes, events block — nginx 顶层参数</p>
<Crud {columns} {fields} load={() => api.basic.list()} save={(row) => api.basic.save(row)} remove={(id) => api.basic.remove(id)} blank={{ name: '', value: '', descr: '' }} />
