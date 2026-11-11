# nginxWebUI

[English summary](README.en.md). License: [Mulan PSL v1](LICENSE) (SPDX `MulanPSL-1.0`).

SvelteKit 静态前端 + Rust 后端 + SQLite。图形界面只改配置并调用本机 `nginx` / `acme.sh`，不常驻 Node、不内置 ACME。

工作方式对齐服务器上的 `script/add-static-site.sh`、`add-sub-site.sh`、`issue-cert.sh`、`issue-gcore-cert.sh`：站点文件写到 `sites-available`，证书装到 `/etc/nginx/ssl`。

## 和原 Java 版的差异（有意简化）

原 Solon/Java 工程已从本仓库移除。完整对照见 [DIFF.md](DIFF.md)。摘要：

| 能力 | 现状 |
| --- | --- |
| 登录 / 管理员 | 有 |
| 静态站 / 反代 / 路径规则 | 有（表单对齐原项目；文件写 Debian sites-available，nginx -t 失败回滚） |
| 静态站 zip 上传 | 有（解压到 /var/www；条目/单文件/合计未压缩大小受限，拒绝 zip-slip） |
| 证书签发续签 | 有（本机 acme.sh，Cloudflare / Gcore DNS） |
| nginx / acme.sh 缺失时安装 | 有（默认关闭自动安装；需在设置页二次确认。Linux 包管理器 + 官方 acme.sh 安装脚本） |
| nginx -t / reload / 启停 | 有（本机 nginx / systemctl） |
| 日志尾部 | 有 |
| SQLite | 有（进程内 rusqlite，无独立数据库服务） |
| HTTP / 基础参数 / 上游 / stream | 保留在 advanced，不再生成整份 nginx.conf |
| 远程集群 / SSO / 邮件告警 | 无 |
| MySQL / PostgreSQL | 无（仅 SQLite） |

## 目录

- `server/` — Rust 进程：JSON API + 托管 `web/build`
- `web/` — SvelteKit（adapter-static）
- `script/` — 本机 nginx/acme 脚本，以及 Linux / Windows 部署启停

## 运行

改仓库根目录的 `nginxwebui.conf`（端口、数据目录、前端路径），然后：

Linux：

```bash
chmod +x script/linux/*.sh
sudo script/linux/setup-sudo.sh   # 一次性：给当前用户免密写 nginx；不授权 apt/dnf，不要 sudo 启动 UI
script/linux/deploy.sh            # 普通用户
```

Windows：

```powershell
script\windows\deploy.ps1
```

Windows 上可以启动本面板（界面和 API）。nginx 路径、`nginx -t` / reload、`sites-available` 以及 acme.sh 仍按 Linux 约定（例如 `/etc/nginx`）；本机不会自动安装或管理这些工具。签发证书和写入站点配置请在 Linux 主机或容器中进行。

在 Windows 上交叉编译 Linux 二进制（musl 静态，不经过 WSL）：

```powershell
script\windows\build-linux.ps1
```

产物在 `dist/linux-x86_64/nginxwebui`。ARM 用 `-Target aarch64-unknown-linux-musl`。说明见 `script/README.md`。

浏览器打开配置里的端口（默认只监听 `http://127.0.0.1:8080`）。远程访问请在 `nginxwebui.conf` 设 `bind = 0.0.0.0`，**不要把面板裸奔在公网**，公网请经 HTTPS 反向代理。数据、日志、pid 默认在 `{仓库}/.run-home`。

进程日志写在 `{home}/logs/nginxwebui.log`。存活检查请打 `GET /healthz`（不返回是否未初始化）。

第一次启动且还没有管理员时，控制台 / 日志会打印一次性初始化令牌（也可用 `NGINXWEBUI_INIT_TOKEN` 指定）。登录页创建管理员必须填写该令牌和验证码。

## Docker

容器内已装 nginx + acme.sh。镜像和 compose **必须**把管理面板绑到 `0.0.0.0`（`bind = 0.0.0.0` / `NGINXWEBUI_BIND=0.0.0.0`），否则端口映射会失效；同时监听 80/443。公网请在前面加 HTTPS 反代。reload 用 `nginx -s reload`。

```bash
docker compose up -d --build
```

管理界面默认映射为宿主机 `http://127.0.0.1:8080`（compose 使用 `127.0.0.1:8080:8080`）。公网请加 HTTPS 反代，不要把 8080 直接打到 `0.0.0.0`。数据在 named volume 里。ARM 机器同样 `docker compose build`（本机架构）。

管理宿主机已有 nginx 时见 `docker-compose.yml` 底部注释：`network_mode: host` 且 `NGINXWEBUI_START_NGINX=0`。

改端口或路径：编辑容器数据卷里的 `nginxwebui.conf`。环境变量仍可覆盖配置文件。更细说明见 `script/README.md`。

开发：先起后端，再 `cd web && pnpm install && pnpm run dev`（`/adminPage` 代理到 8080）。

## 本机约定

本机已有 `/etc/nginx/nginx.conf` 时，默认使用 Debian 布局：

- `/etc/nginx/sites-available`、`sites-enabled`
- `/etc/nginx/ssl/<域名>.cer` / `.key`
- reload：`sudo systemctl reload nginx`

Cloudflare / Gcore 凭据写在设置页，或事先保存在 `~/.acme.sh/account.conf`。

## 从旧版 / 已有环境迁移

启动时会扫描本机，**不覆盖**已有 nginx.conf（仅在缺少 `sites-enabled` include 时补一行）：

- 已安装的 nginx：`nginx -V` 的 `--conf-path` / `--prefix`，以及 `/usr/sbin/nginx`
- 证书：`/etc/nginx/ssl/*.cer`、certbot `fullchain.pem`、旧版 `{project.home}/.acme.sh/<域名>/fullchain.cer`、`{project.home}/cert/<域名>/certificate.pem`、现有站点里的 `ssl_certificate`
- acme.sh：`~/.acme.sh`、`/root/.acme.sh`、旧版 `/home/nginxWebUI/.acme.sh`、当前 `--project.home/.acme.sh`
- `sites-available` / `conf.d` 里已有的 server，会进站点列表并标「已有」

已由 acme.sh 签发的证书，续签走 `acme.sh --renew`（含 ECC），沿用原来的 DNS/HTTP 验证，不会改成 Cloudflare。`install-cert` 优先写回 nginx 里已经在用的 pem/key 路径。删除证书记录不会删除 `.acme.sh`、Let's Encrypt、旧版 `cert/` 里的文件。

站点列表里对「已有」站点点应用只会 `nginx -t` 并 reload，不改原文件。在编辑页点「写入并 reload」才会按新模板生成。配置页显示磁盘上的 nginx.conf；对不是本程序生成的主 conf，拒绝一键覆盖。reload 会先跑 `nginx -t`。

把 `--project.home` 指到旧 Java 版的数据目录即可带上那边的 acme 与上传证书。旧版 H2 库不导入；以磁盘上的 nginx / acme 文件为准。
