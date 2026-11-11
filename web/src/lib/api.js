/* A thin wrapper over fetch. Every call goes to the Rust server under /adminPage. */

let unauthorized = () => {};

/** Called when the server says the session is gone (401 + needLogin). */
export function onUnauthorized(fn) {
  unauthorized = fn;
}

function query(params) {
  if (!params) return '';
  const parts = [];
  for (const key of Object.keys(params)) {
    const value = params[key];
    if (value === undefined || value === null || value === '') continue;
    parts.push(encodeURIComponent(key) + '=' + encodeURIComponent(value));
  }
  return parts.length ? '?' + parts.join('&') : '';
}

async function call(path, init) {
  const res = await fetch(path, { credentials: 'same-origin', ...init });
  if (res.status === 401) {
    unauthorized();
    throw new Error('needLogin');
  }
  const text = await res.text();
  let data;
  try {
    data = text ? JSON.parse(text) : {};
  } catch {
    return { success: false, msg: text.slice(0, 200) };
  }
  return data;
}

export function get(path, params) {
  return call(path + query(params));
}

export function post(path, body) {
  return call(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body || {})
  });
}

const table = (name) => ({
  list: () => get(`/adminPage/${name}/list`),
  save: (row) => post(`/adminPage/${name}/save`, row),
  remove: (id) => post(`/adminPage/${name}/del`, { id })
});

export const api = {
  login: {
    info: () => get('/adminPage/login/info'),
    code: () => get('/adminPage/login/getCode'),
    signIn: (name, pass, code) => post('/adminPage/login/login', { name, pass, code }),
    addAdmin: (name, pass, code, initToken) =>
      post('/adminPage/login/addAdmin', { name, pass, code, initToken }),
    changePass: (oldPass, newPass) => post('/adminPage/login/changePass', { oldPass, newPass }),
    logout: () => post('/adminPage/login/loginOut', {})
  },
  monitor: () => get('/adminPage/main/monitor'),
  basic: table('basic'),
  http: table('http'),
  stream: table('stream'),
  denyAllow: table('denyAllow'),
  password: table('password'),
  cert: {
    ...table('cert'),
    list: () => get('/adminPage/cert/list'),
    scan: () => post('/adminPage/cert/scan', {}),
    apply: (id, force = false) => post('/adminPage/cert/apply', { id, force }),
    renew: (id, force = true) => post('/adminPage/cert/renew', { id, force }),
    cron: () => post('/adminPage/cert/cron', {}),
    progress: () => get('/adminPage/cert/progress'),
    status: () => get('/adminPage/cert/status')
  },
  site: {
    list: () => get('/adminPage/site/list'),
    detail: (id) => get('/adminPage/site/detail', { id }),
    save: (row) => post('/adminPage/site/save', row),
    remove: (id) => post('/adminPage/site/del', { id }),
    apply: (id, extra = {}) => post('/adminPage/site/apply', { id, ...extra }),
    preview: (row) => post('/adminPage/site/preview', row),
    findCert: (domain) => get('/adminPage/site/findCert', { domain })
  },
  www: {
    list: () => get('/adminPage/www/list'),
    save: (row) => post('/adminPage/www/save', row),
    remove: (id) => post('/adminPage/www/del', { id }),
    clean: (id) => post('/adminPage/www/clean', { id }),
    upload: async (dir, file, descr) => {
      const fd = new FormData();
      fd.append('dir', dir);
      if (descr) fd.append('descr', descr);
      if (file) fd.append('file', file);
      return call('/adminPage/www/upload', { method: 'POST', body: fd });
    }
  },
  install: {
    status: () => get('/adminPage/install/status'),
    nginx: () => post('/adminPage/install/nginx', {}),
    acme: () => post('/adminPage/install/acme', {}),
    auto: (enable) => post('/adminPage/install/auto', { enable })
  },
  template: table('template'),
  server: {
    ...table('server'),
    list: (params) => get('/adminPage/server/list', params),
    detail: (id) => get('/adminPage/server/detail', { id }),
    save: (row) => post('/adminPage/server/save', row),
    enable: (id, enable) => post('/adminPage/server/enable', { id, enable })
  },
  upstream: {
    list: () => get('/adminPage/upstream/list'),
    detail: (id) => get('/adminPage/upstream/detail', { id }),
    save: (upstream, nodes) => post('/adminPage/upstream/save', { upstream, nodes }),
    remove: (id) => post('/adminPage/upstream/del', { id })
  },
  conf: {
    show: () => get('/adminPage/conf/show'),
    replace: (content) => post('/adminPage/conf/replace', { conf: content }),
    check: () => post('/adminPage/conf/check', {}),
    reload: () => post('/adminPage/conf/reload', {}),
    signal: (cmd) => post('/adminPage/conf/signal', { action: cmd }),
    status: () => get('/adminPage/conf/status'),
    saveCmd: (body) => post('/adminPage/conf/saveCmd', body),
    write: () => post('/adminPage/conf/write', {}),
    sitesInclude: () => get('/adminPage/conf/sitesInclude'),
    applySitesInclude: () => post('/adminPage/conf/sitesInclude', { confirm: true })
  },
  bak: {
    list: () => get('/adminPage/bak/list'),
    detail: (id) => get('/adminPage/bak/detail', { id }),
    restore: (id) => post('/adminPage/bak/restore', { id })
  },
  log: {
    names: () => get('/adminPage/log/names'),
    tail: (name, lines) => get('/adminPage/log/tail', { name, lines }),
    operate: () => get('/adminPage/log/operate')
  },
  admin: table('admin'),
  setting: {
    get: (key) => get('/adminPage/setting/get', { key }),
    set: (key, value) => post('/adminPage/setting/set', { key, value })
  }
};
