# 部署 / 启停脚本

生产环境：**前端是静态文件**，由 Rust 后端托管。没有独立的前端常驻进程。

- `deploy-web`：`pnpm run build` → `web/build`
- `deploy-server`：`cargo build --release`，拷贝二进制后重启后端
- `start` / `stop` / `restart`：后端进程（默认端口 8080）
- `start-web` / `stop-web`：可选的 Vite 开发服务（5173），生产不必用

数据目录默认 `{仓库}/.run-home`（库、日志、pid、安装的二进制）。

可用环境变量覆盖：`NGINXWEBUI_PORT`、`NGINXWEBUI_HOME`、`NGINXWEBUI_WEB_DIR`。

## Linux

```bash
chmod +x script/linux/*.sh
script/linux/deploy.sh          # 前后端一起构建并启动
script/linux/deploy-web.sh      # 只构建前端
script/linux/deploy-server.sh   # 只构建并重启后端
script/linux/start.sh           # 启动后端
script/linux/restart.sh         # 重启后端
script/linux/stop.sh            # 停止后端（以及开发用 Vite）
```

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
./nginxwebui --port 8080 --project.home /home/nginxWebUI
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

