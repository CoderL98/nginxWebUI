# 出站 Agent 注册中心

一台面板同时作为注册中心。被控主机运行 `nginxwebui-agent`，**只主动出站**连接面板；被控端不监听 TCP，也不需要开放任何入站防火墙端口。本机永远是默认节点，面板所在机器不必再跑一份 agent。

之所以用 HTTP 长轮询而不是 SSH 或 WebSocket：面板进程是 `tiny_http` + 少量工作线程，没有把整个 HTTP 栈改成 async。Agent 用同步 HTTP 客户端（`ureq` + rustls）定时 `POST /agent/v1/poll`。为避免几个 agent 卡住面板，长轮询请求会把 `tiny_http::Request` 交给独立的 agent 线程池等待任务或超时，工作线程立刻回去接受新连接。

## 架构

```
管理员浏览器  --(会话 Cookie)-->  面板 / 注册中心
                                      ^
                                      | 出站 HTTPS 长轮询
                                 nginxwebui-agent（各节点）
                                      |
                                 本机 nginx / acme.sh / nginxwebui-priv
```

1. 在「设备」页创建设备，得到一次性加入令牌（明文只显示一次，哈希入库，默认 24 小时过期）。
2. 被控端执行安装命令或 `nginxwebui-agent join --url … --token …`，成功后获得长期节点凭证（同样只存哈希），旧加入令牌作废。
3. `nginxwebui-agent run` 循环：hello / 长轮询拉任务 / 回传结果；约 25 秒无任务则重轮询，断线按退避重连。
4. 管理员在顶栏切换当前设备。缺省为本机，走原来的本机逻辑。选中远程节点时，站点 / 证书申请 / 日志 / reload 等请求带 `X-NginxWebUI-Node`，由面板编成白名单 RPC 交给该 agent。

## 安全模型

- **不是远程 shell**。Agent 只执行白名单方法：`ping`、`sysinfo`、`nginx.status`、`nginx.test`、`nginx.reload`、`nginx.signal`、`site.list`、`site.detail`、`site.preview`、`site.apply`、`conf.show`、`log.tail`、`acme.status`、`acme.apply`。其它方法返回 `unsupportedMethod`。
- 写配置和 reload 复用现有 `nginxwebui-priv`，agent 不以 root 为设计前提。
- 加入令牌与节点凭证只存 SHA-256 哈希；列表、导出不会回传明文。撤销节点会立即作废凭证并断开等待中的任务。
- TLS 默认校验证书。可用 `--tls-ca` 指向额外的 CA 文件（当前客户端以系统/内置根证书校验为主，自定义 CA 请并入反代或系统信任库）、或仅在受控环境使用 `--insecure` / `http://`。`--tls-fingerprint` 会被记录，但此 HTTP 客户端无法读取对端证书，不能单独完成钉扎。
- 面板自己的设置、管理员、备份、导出不会当成远程操作下发。

## 安装一台被控端

面板必须能被 agent 访问。多机模式下请把面板放在 HTTPS 反代后面（或至少 `bind` 到 agent 能连上的地址），**不要**只监听 `127.0.0.1` 却指望其它机器加入。被控端只需能出站访问该 URL。

1. 打开面板「设备」，填写名称并添加。
2. 复制一次性安装命令，例如：

```bash
curl -fsSL https://panel.example/agent/install.sh | sh -s -- --url https://panel.example --token <token>
```

3. 脚本会尝试从面板下载与面板同架构的 `nginxwebui-agent`。若面板旁没有该二进制（常见于尚未交叉编译的环境），请先在 Linux 上执行：

```bash
cd server && cargo build --release --bin nginxwebui-agent
# 把 target/release/nginxwebui-agent 拷到被控端后：
./nginxwebui-agent join --url https://panel.example --token <token>
./nginxwebui-agent run
```

生产环境请用 systemd/supervisor 常驻 `run`。配置默认写在 `~/.config/nginxwebui-agent/agent.json`（可用 `--home` 或 `NGINXWEBUI_AGENT_HOME` 修改）。

4. 设备变为在线后，在顶栏切换到该节点，再使用站点、证书、配置检查/reload、日志等页面。

## 限制

- 第一版 agent 面向 Linux。Windows 可以编译二进制，但不作为被控端交付：nginx / acme.sh 仍按 Linux 路径约定。
- `/agent/download` 只提供与面板进程相同的 OS/架构；不做多架构交叉编译托管。
- 远程不支持面板本地的 www 上传、上游/stream 编辑、设置项、备份恢复等未映射到白名单的功能。
- 长轮询超时约 25 秒，远程 RPC 等待约 30 秒；节点超过约 90 秒无心跳视为离线。
