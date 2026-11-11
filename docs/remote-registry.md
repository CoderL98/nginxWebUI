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
| 安装状态 `install/status`，安装 `install/nginx`、`install/acme`，自动安装开关 `install/auto` | `install.status` / `install.nginx` / `install.acme` / `install.auto` | 支持（需确认；`autoInstall` 默认关闭；acme.sh 使用固定版本，见下） |
| 登录、管理员、设备、设置、导出、备份、面板操作日志 | — | 始终在面板本地执行 |
| www 上传、上游 / stream 等其它 | — | 返回 `notSupportedOnNode` |

旧方法名 `acme.status` / `acme.apply` 仍被 agent 接受。旧版 agent 返回的 `{ ok, output }` 会被面板规范成新结构。**建议面板与 agent 使用同一版本。**

节点离线时远程请求立即返回 `nodeOfflineErr`，不再等 30 秒。只有写操作（save/apply/del/reload 等）会写入面板操作日志；`conf/check`、`reload` 以 nginx 实际结果记录成败。

## 被控端前置条件

- 被控端可以预先安装 nginx / acme.sh，也可以在选中该设备后于「设置」页确认安装。acme.sh 使用仓库钉死的 GitHub 标签（见 `script/acme.sh.pin`），不会再 `curl | sh` 一条会变动的 URL。
- Agent 启动时与面板一样扫描本机：有 `/etc/nginx/nginx.conf` 就用 Debian 布局，否则落在 agent home 下；补 `sites-enabled` include；启动日志打印实际使用的 nginx.conf、sites 目录和 reload 命令，配置页也会显示「实际路径」。
- Agent 自带证书自动续签定时任务（与面板本机相同）。
- Cloudflare / Gcore 凭据和 acme 邮箱在**选中该设备后**的证书页 DNS 卡片里填写，保存在该节点，不与面板共享。

## 安全模型

- **不是远程 shell**。Agent 只执行上表白名单方法，其它返回 `unsupportedMethod`。写配置和 reload 复用现有 `nginxwebui-priv`。
- 加入令牌与节点凭证只存 SHA-256 哈希；凭证只从 `Authorization: Bearer` 头读取，URL 查询参数里的凭证一律 401。
- 设备凭证存在 SQLite 里，**不会出现在导出 JSON / 备份里**；导入数据时保留现有设备的凭证哈希、证书密钥和密码类设置，忽略导入文件中的同名秘密字段。
- 「轮换加入令牌」对未加入或已加入的设备签发新的一次性令牌（24 小时，只显示一次）；已加入设备的旧凭证在新 `join` 成功前仍然有效。撤销会立即作废凭证并断开等待中的任务。已撤销的设备不必删除，点「重新接入」即可签发新令牌、清除撤销状态，然后重新 `join`。
- 同一凭证被两台主机同时使用时，新的轮询会顶替旧的（`pollSuperseded`），设备列表标记「凭证冲突」，agent 日志提示 agent.json 可能被复制。
- `agent.json` 以 0600 原子写入。也可用环境变量 `NGINXWEBUI_AGENT_CREDENTIAL` 提供凭证，不落盘。
- TLS 始终校验证书。`--insecure` 已移除（使用会报错）。`--tls-ca FILE` 把 PEM CA 加入客户端信任库（同时保留 webpki 根证书）。`--tls-fingerprint` 钉死面板**叶证书**的 SHA-256（64 位十六进制，允许夹杂冒号或空格，也可写成 `sha256:` 前缀）；不一致则拒绝连接。只给指纹、不给 CA 时可用于自签证书。两者都不设时走默认校验。对非回环地址使用 `http://` 时 agent 会告警。
- 面板可选内置 HTTPS：配置 `tls_cert` / `tls_key`（PEM），或环境变量 `NGINXWEBUI_TLS_CERT` / `NGINXWEBUI_TLS_KEY`，或 `--tls-cert` / `--tls-key`。未设置时仍是 HTTP（默认绑 `127.0.0.1`）。直接提供 HTTPS 时会话 Cookie 会自动加 Secure，启动日志打印 `https://...`。生产环境通常仍建议放在反代后面。
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

Windows（管理员 PowerShell，会安装 Windows 服务）：

```powershell
irm https://panel.example/agent/install.ps1 -OutFile $env:TEMP\nginxwebui-agent-install.ps1
powershell -ExecutionPolicy Bypass -File $env:TEMP\nginxwebui-agent-install.ps1 -Url https://panel.example -Token <token>
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

   手动部署可参考 `script/linux/nginxwebui-agent.service`。配置默认写在 `~/.config/nginxwebui-agent/agent.json`（`--home` 或 `NGINXWEBUI_AGENT_HOME` 可改）。自签或私有 CA 时加上 `--tls-ca FILE` 或 `--tls-fingerprint HEX`。

## Agent 二进制布局与自动升级

`/agent/download?os=&arch=` 优先提供 `agents/{os}-{arch}/nginxwebui-agent`（例如 `agents/linux-x86_64/`、`agents/linux-aarch64/`），没有时再回退到面板同目录、与面板相同架构的 `nginxwebui-agent`。安装脚本会按加入主机的 os/arch 请求，Windows 提供 `nginxwebui-agent.exe`（`agents/windows-x86_64/`）。把其它架构的 agent 拷进 `agents/` 即可由本机面板托管，不必做多架构 Docker 镜像。Windows 被控端需要 Windows 版 nginx；证书签发通过 Git Bash 或 WSL 运行钉死版本的 acme.sh；请以对 nginx 目录有写权限的账户安装 Windows 服务。自动升级在 Windows 上会先把正在运行的 exe 改名为 `.old` 再写入新文件，服务退出后由 SCM 拉起，下次启动再删除 `.old`。

加入之后，hello 会带上面板侧对应 os/arch 的 agent 版本和 sha256。默认在「已校验的 HTTPS」或「回环 http」时，若版本不同则从同一面板的 `/agent/download` 下载、校验哈希、原子替换二进制并 re-exec（systemd 的 `Restart=always` 也能拉起来）。哈希对不上会拒绝安装。版本已相同则跳过。可用 `--no-upgrade`、`agent.json` 里 `"upgrade": false`，或 `NGINXWEBUI_AGENT_NO_UPGRADE=1` 关闭。**不会**从第三方 URL 下载。

## 完整同步与同步组

「设备」页的手动同步会复制选中站点、关联证书记录、源节点 `sslDir` 下的证书/私钥文件（文件名经过净化，不含路径分隔符），以及本机已有的 DNS 凭据。写入目标后先执行 `nginx -t`，通过才 reload；测试失败时不会让正在运行的残缺配置生效。源是远程设备时也会通过白名单 RPC（`ssl.list` / `ssl.get`）读取证书文件；DNS 密钥仍只写不回显，需在目标证书页自行填写。证书文件内容不会出现在预览、列表接口或操作日志里。

同步组保存在面板的 SQLite 中（不在 agent 上）：

- **双活（active-active）**：任一成员上成功应用站点 / 申请或续签证书 / 完成手动同步后，自动把记录和证书文件同步到其他*在线*成员，并在目标上 `nginx -t` + reload。离线成员会跳过并记入结果。一次同步带有 wave id，避免互相复制形成死循环。
- **主备（primary-standby）**：只有当前主节点的变更会传到备机（热备）。主节点离线（约 90 秒无心跳；本机始终视为在线）时，自动提升一台健康备机，写入操作日志并在界面显示；也可以手动提升。
- 负载均衡健康检查（无需登录）：`GET /healthz` 仍只表示面板进程存活。`GET /healthz/group/{id}`（指本机成员）和 `GET /healthz/group/{id}/node/{nodeId}` 仅在该成员**应当接流量**时返回 200（双活：成员在线；主备：该成员是当前主且在线），否则 503，组不存在则 404。请把 nginx / HAProxy / 外部 LB 的后端探活指向这些地址。可选的 advertise IP 仅供展示/文档，本面板**不会**配置或漂移浮动 IP、VRRP 或 ARP。

## acme.sh 固定版本

本机和远程安装、Docker 镜像都使用 `script/acme.sh.pin` 钉死的 GitHub 标签（当前 3.1.6），下载后先校验 sha256 再解压执行 `--install`。升级钉选：到 [acme.sh releases](https://github.com/acmesh-official/acme.sh/releases) 选标签（3.1.6 起有签名），下载 archive，`sha256sum`，更新 `script/acme.sh.pin` 和 `server/src/install.rs` 里的 `ACME_SH_*` 常量。

4. 设备变为在线后（列表每 10 秒刷新），在顶栏切换到该节点使用各页面。远程模式下页面顶部有提示条，仅本机可用的功能会隐藏或标「仅面板」。

## 限制

- Agent 同时支持 Linux 与 Windows 被控端。Windows 需要 Windows 版 nginx；acme.sh 通过 Git Bash 或 WSL 运行（找不到 bash 时安装/状态会明确提示）。面板仍可在 Windows 上运行。
- 同步组可自动同步配置和证书文件，并为主备提供自动提升与 LB 健康检查；**不会**移动虚拟 IP / 做 VRRP。
- `--insecure` 不会恢复。未配置 `tls_cert` / `tls_key` 时面板仍是 HTTP（默认 `127.0.0.1`）；多机生产环境通常仍建议 HTTPS 反代。
- Docker：面板进程以 `nginxwebui` 用户运行，acme 家目录在 `/home/nginxWebUI/.acme.sh`。监听 80/443 的 nginx 主进程仍可能是特权进程。
- 仓库根目录 `LICENSE` 是木兰宽松许可证第 1 版（SPDX `MulanPSL-1.0`）。GitHub 网页上的 License 字段可能仍显示 Other / NOASSERTION，因为 GitHub 常用许可证列表往往不含木兰，不要为此改成一个不真实的 license key。
- 长轮询超时约 25 秒，远程 RPC 等待约 30 秒；节点超过约 90 秒无心跳视为离线。
- 设置、管理员、导出和面板备份仍只作用于面板本机。
