# 部署 / 启停脚本

生产环境：**前端是静态文件**，由 Rust 后端托管。没有独立的前端常驻进程。

- `deploy-web`：`pnpm run build`，再把 `web/build` 同步到配置里的 `web_dir`
- `deploy-server`：`cargo build --release`，拷贝二进制后重启后端
- `start` / `stop` / `restart`：后端进程（默认端口 8080）
- `start-web` / `stop-web`：可选的 Vite 开发服务（5173），生产不必用

数据目录默认 `{仓库}/.run-home`（库、日志、pid、安装的二进制）。

**改端口 / 路径：编辑仓库根目录 `nginxwebui.conf`，然后 `restart`。**

```
port = 9000
home = .run-home
web_dir = .run-home/web
```

相对路径相对于配置文件所在目录。优先顺序：命令行 > 环境变量 > 配置文件 > 默认值。也可设 `NGINXWEBUI_CONFIG` 指向另一份文件。

## Linux

```bash
chmod +x script/linux/*.sh
sudo script/linux/setup-sudo.sh # 一次性：助手 + sudoers（不要 sudo 启动 UI）
script/linux/deploy.sh          # 前后端一起构建并启动（普通用户）
script/linux/deploy-web.sh      # 只构建前端
script/linux/deploy-server.sh   # 只构建并重启后端
script/linux/start.sh           # 启动后端
script/linux/restart.sh         # 重启后端
script/linux/stop.sh            # 停止后端（以及开发用 Vite）；会清 pid 文件对不上的残留进程
```

**不要用 `sudo script/linux/start-server.sh` / `deploy.sh`。** 整个管理进程以 root 跑时，界面被打穿就是整机 root，pid/日志/数据目录也会变成 root 属主。

正确做法：进程用当前用户启动。`setup-sudo.sh` 安装 root 属主的 `/usr/local/sbin/nginxwebui-priv`（只允许改 `/etc/nginx`、`/var/www`、`/var/log/nginx`），并给该用户免密执行助手、`systemctl reload nginx`、`nginx -t`。启动脚本若检测到 EUID=0 会退出（容器或 `NGINXWEBUI_ALLOW_ROOT=1` 除外）；若写不了 `/etc/nginx` 且助手未就绪也会退出。

## Windows

PowerShell（建议）：

```powershell
script\windows\deploy.ps1
script\windows\deploy-web.ps1
script\windows\deploy-server.ps1
script\windows\start.ps1
script\windows\restart.ps1
script\windows\stop.ps1
```

或双击 / cmd：`deploy.cmd`、`start.cmd`、`restart.cmd`、`stop.cmd`。

## 在 Windows 上编 Linux 二进制

不经过 WSL。用 Zig 做链接器，默认编 **musl 静态** 包，拷到 Debian / Ubuntu / RHEL 即可跑：

```powershell
script\windows\build-linux.ps1
```

产物：`dist/linux-x86_64/nginxwebui`。

```powershell
script\windows\build-linux.ps1 -Target aarch64-unknown-linux-musl
script\windows\build-linux.ps1 -Target x86_64-unknown-linux-gnu
```

第一次会安装 `cargo-zigbuild`，并在 `%LOCALAPPDATA%\nginxwebui-tools\zig` 放下便携 Zig（PATH 里已有 `zig` 则直接用）。`rusqlite` 已 bundled SQLite，交叉编译不需要 Linux 上的 libsqlite。

把文件拷到服务器后：

```bash
chmod 755 nginxwebui
# 把 nginxwebui.conf 和二进制放一起，或：
./nginxwebui --config /etc/nginxwebui.conf
```

## Docker

仓库根目录：

```bash
docker compose up -d --build
```

- 管理端口 8080（已绑定 0.0.0.0）
- 容器内 nginx 占用 80/443
- reload：`nginx -s reload`
- 数据：`nginxwebui-data` 等 named volume

管理宿主机 nginx 时不要在容器里再起一份 nginx，见根目录 `docker-compose.yml` 注释。

