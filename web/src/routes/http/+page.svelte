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
    { key: 'value', label: 'value', type: 'area', rows: 2 },
    { key: 'descr', label: 'remark' }
  ];
</script>

<h1 class="mb-1 text-lg font-semibold">{$tt('http')}</h1>
<p class="text-muted-foreground mb-4 text-sm">http block — gzip、超时、client_max_body_size</p>
<p class="text-muted-foreground mb-4 text-sm">{$tt('siteVsAdvanced')}</p>
<Crud {columns} {fields} load={() => api.http.list()} save={(row) => api.http.save(row)} remove={(id) => api.http.remove(id)} blank={{ name: '', value: '', descr: '' }} />
