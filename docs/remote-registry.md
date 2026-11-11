# 出站 Agent 注册中心

一台面板同时作为注册中心。被控主机运行 `nginxwebui-agent`，**只主动出站**连接面板；被控端不监听 TCP，也不需要开放任何入站防火墙端口。本机永远是默认节点，面板所在机器不必再跑一份 agent。

之所以用 HTTP 长轮询而不是 SSH 或 WebSocket：面板进程是 `tiny_http` + 少量工作线程，没有把整个 HTTP 栈改成 async。Agent 用同步 HTTP 客户端（`ureq` + rustls）定时 `POST /agent/v1/poll`。长轮询请求会交给独立的 agent 线程池（`agent_threads`，默认 32）等待任务或超时，工作线程立刻回去接受新连接。

## 架构

```
管理员浏览器  --(会话 Cookie + X-Requested-With)-->  面板 / 注册中心
                                      ^
                                      | 出站 HTTPS 长轮询
                                 nginxwebui-agent（各节点）
                                      |
                                 本机 nginx / acme.sh / nginxwebui-priv
```

1. 在「设备」页填写**面板地址**（agent 能访问到的 URL，建议 HTTPS），再创建设备，得到一次性加入令牌（明文只显示一次，哈希入库，默认 24 小时过期）。
2. 被控端执行安装命令或 `nginxwebui-agent join --url … --token …`，成功后获得长期节点凭证（同样只存哈希），加入令牌作废。
3. `nginxwebui-agent run` 循环：hello / 长轮询拉任务 / 回传结果；约 25 秒无任务则重轮询，断线按退避重连。
4. 管理员在顶栏切换当前设备（小屏也可见）。缺省为本机。选中远程节点时，站点 / 证书 / 配置检查与 reload / 日志等请求带 `X-NginxWebUI-Node`，由面板编成白名单 RPC 交给该 agent。

Agent 收到 RPC 后，直接调用**与面板本机完全相同的 API 处理器**（参数格式 `{query, body}`），所以远程和本机的返回结构、字段合并、校验逻辑一致。例如「按 id 应用站点」不会再把库里的行清空，`conf/check` 返回 `{ success, output }`。

## 远程映射表

| 页面 / 接口 | RPC 方法 | 远程 |
| --- | --- | --- |
| 监控 `main/monitor` | `sysinfo` | 支持 |
| 配置 `conf/show`、`status`、`check`、`reload`、`signal` | `conf.show`、`nginx.status/test/reload/signal` | 支持（主配置编辑 / 替换仅本机） |
| 站点 `list/detail/preview/findCert/save/apply/del` | `site.*` | 支持 |
| 日志 `names`、`tail` | `log.names`、`log.tail` | 支持 |
| 证书 `list/scan/status/progress/save/del/apply/renew/cron` | `cert.*` | 支持 |
| 证书 DNS 凭据 `cert/dns`、`cert/dnsSave` | `cert.dns`、`cert.dnsSave` | 支持，**按设备保存**，只写不回显 |
| 安装状态 `install/status` | `install.status` | 只读；远程不执行安装 |
| 登录、管理员、设备、设置、导出、备份、面板操作日志 | — | 始终在面板本地执行 |
| www 上传、上游 / stream 等其它 | — | 返回 `notSupportedOnNode` |

旧方法名 `acme.status` / `acme.apply` 仍被 agent 接受。旧版 agent 返回的 `{ ok, output }` 会被面板规范成新结构。**建议面板与 agent 使用同一版本。**

节点离线时远程请求立即返回 `nodeOfflineErr`，不再等 30 秒。只有写操作（save/apply/del/reload 等）会写入面板操作日志；`conf/check`、`reload` 以 nginx 实际结果记录成败。

## 被控端前置条件

- 被控端需**预先安装 nginx 和 acme.sh**（远程不会执行 apt/curl 安装）。
- Agent 启动时与面板一样扫描本机：有 `/etc/nginx/nginx.conf` 就用 Debian 布局，否则落在 agent home 下；补 `sites-enabled` include；启动日志打印实际使用的 nginx.conf、sites 目录和 reload 命令，配置页也会显示「实际路径」。
- Agent 自带证书自动续签定时任务（与面板本机相同）。
- Cloudflare / Gcore 凭据和 acme 邮箱在**选中该设备后**的证书页 DNS 卡片里填写，保存在该节点，不与面板共享。

## 安全模型

- **不是远程 shell**。Agent 只执行上表白名单方法，其它返回 `unsupportedMethod`。写配置和 reload 复用现有 `nginxwebui-priv`。
- 加入令牌与节点凭证只存 SHA-256 哈希；凭证只从 `Authorization: Bearer` 头读取，URL 查询参数里的凭证一律 401。
- 设备凭证存在 SQLite 里，**不会出现在导出 JSON / 备份里**；导入数据时保留现有设备的凭证哈希、证书密钥和密码类设置，忽略导入文件中的同名秘密字段。
- 「轮换加入令牌」只针对未加入 / 待加入的设备：生成新令牌会作废旧令牌，已加入设备的长期凭证不受影响。撤销会立即作废凭证并断开等待中的任务；被撤销的设备要重新接入请删除后重新添加。
- 同一凭证被两台主机同时使用时，新的轮询会顶替旧的（`pollSuperseded`），设备列表标记「凭证冲突」，agent 日志提示 agent.json 可能被复制。
- `agent.json` 以 0600 原子写入。也可用环境变量 `NGINXWEBUI_AGENT_CREDENTIAL` 提供凭证，不落盘。
- TLS 始终校验证书。`--insecure` 已移除（使用会报错）。自定义 CA 请加入系统信任库或放在受信反代后面。`--tls-ca` / `--tls-fingerprint` 目前只记录，不强制。对非回环地址使用 `http://` 时 agent 会告警。
- 面板接入面：`/agent/install.sh` 每 IP 每分钟 30 次、`/agent/download` 10 次、`/agent/v1/join` 10 次，超出返回 429；请求体限制为登录 16KB、`/agent/v1/result` 4MB、其它 `/agent/*` 64KB、后台接口 16MB，超出返回 413。
- 后台写接口要求请求头 `X-Requested-With: nginxwebui`（自带前端会自动发送），缺失返回 403 `csrfRequired`。自己写脚本调后台 API 时请加上这个头。

## 反向代理与 trusted_proxies

默认**不信任** `X-Forwarded-For` / `X-Forwarded-Proto`，限流和安装命令里的地址都按 TCP 对端计算。面板在反代后面时，在 `nginxwebui.conf` 设置：

```
trusted_proxies = 127.0.0.1/32, ::1/128
```

（或环境变量 `NGINXWEBUI_TRUSTED_PROXIES`、参数 `--trusted-proxies`。）Docker 里反代通常经网关进来，可用 `172.16.0.0/12`；请按实际网段收紧。只有来自这些地址的请求才会采信转发头，客户端 IP 取 XFF 中最右侧的非可信地址。

`agent_threads`（`NGINXWEBUI_AGENT_THREADS` / `--agent-threads`，默认 32）是同时挂起的长轮询上限，超出返回 `tooManyAgents`；设备多时调大。

## 安装一台被控端

面板必须能被 agent 访问。请在「设备」页设置面板地址（存为设置项 `agentPanelUrl`），否则安装命令用浏览器当前地址：若是 `127.0.0.1` / `localhost` 会提示「回环地址」，若是非回环的 `http://` 会提示「不安全」。

1. 打开面板「设备」，确认面板地址，填写名称并添加。
2. 复制一次性安装命令，例如：

```bash
curl -fsSL https://panel.example/agent/install.sh | sh -s -- --url https://panel.example --token <token>
```

   - 以 root 在 systemd 主机上运行（默认）：安装到 `/usr/local/bin/nginxwebui-agent`，配置在 `/var/lib/nginxwebui-agent`（0700，agent.json 0600），写入并启用 `nginxwebui-agent.service`（`Restart=always`，`UMask=0077`）。默认以 root 运行；可用 `--user NAME` 改为普通用户，此时写配置 / reload 需配好 `nginxwebui-priv`。
   - 非 root 或无 systemd，或加 `--foreground`：下载到当前目录并前台运行。
   - 对非回环的 `http://` 面板，脚本默认拒绝；确需在可信内网使用时设 `NGINXWEBUI_ALLOW_HTTP=1`。

3. 发行包和 Docker 镜像已附带 `nginxwebui-agent`（与面板同目录 / `/opt/nginxwebui/`），`/agent/download` 直接提供。自行编译：

```bash
cd server && cargo build --release --bin nginxwebui-agent
./nginxwebui-agent join --url https://panel.example --token <token>
./nginxwebui-agent run
```

   手动部署可参考 `script/linux/nginxwebui-agent.service`。配置默认写在 `~/.config/nginxwebui-agent/agent.json`（`--home` 或 `NGINXWEBUI_AGENT_HOME` 可改）。

4. 设备变为在线后（列表每 10 秒刷新），在顶栏切换到该节点使用各页面。远程模式下页面顶部有提示条，仅本机可用的功能会隐藏或标「仅面板」。

## 限制

- Agent 面向 Linux；Windows 可编译但不作为被控端交付。
- `/agent/download` 只提供与面板相同 OS/架构的二进制；不做多架构托管。
- 面板本身不内置 TLS，多机请放在 HTTPS 反代后面。Docker 镜像以 root 运行。
- 不支持自动升级 agent、节点间配置同步或故障转移。
- 远程不执行 nginx / acme.sh 安装；acme.sh 的本机安装方式仍是官方 `curl | sh`。
- 长轮询超时约 25 秒，远程 RPC 等待约 30 秒；节点超过约 90 秒无心跳视为离线。
