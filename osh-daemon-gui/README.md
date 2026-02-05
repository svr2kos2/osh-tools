# OSH Daemon GUI

GUI版本的OSH Daemon，提供系统托盘驻留和图形化管理界面。

## 功能特性

- ✅ 系统托盘常驻图标
- ✅ 右键菜单控制守护进程启动/停止
- ✅ 配置文件快捷编辑
- ✅ Windows开机自启动
- ✅ 后台运行，无控制台窗口
- ✅ 日志文件记录

## 构建

```bash
# 开发构建
cargo build -p osh-daemon-gui

# 发布构建
cargo build -p osh-daemon-gui --release
```

## 使用方法

1. **首次运行**：双击运行 `osh-daemon-gui.exe`，会在系统托盘显示图标

2. **配置守护进程**：
   - 右键托盘图标 -> "配置"
   - 编辑 `daemon.toml` 文件
   - 设置服务器地址和密钥

3. **启动守护进程**：
   - 右键托盘图标 -> "启动守护进程"

4. **开机自启**：
   - 右键托盘图标 -> "开机自启"（再次点击取消）

5. **退出程序**：
   - 右键托盘图标 -> "退出"

## 配置文件位置

- Windows: `%APPDATA%\osh-daemon\daemon.toml`
- Linux: `~/.config/osh-daemon/daemon.toml`
- macOS: `~/Library/Application Support/osh-daemon/daemon.toml`

## 日志文件位置

日志文件保存在配置目录的 `gui.log` 中。

## 技术栈

- **UI框架**: tray-icon (系统托盘)
- **事件循环**: winit
- **异步运行时**: tokio
- **核心逻辑**: osh-daemon 库

## 开发说明

GUI版本通过引用 `osh-daemon` 库来复用核心功能，无需重复实现。

## 与CLI版本对比

| 特性 | CLI版本 | GUI版本 |
|------|---------|---------|
| 使用场景 | 服务器/命令行环境 | 桌面环境 |
| 启动方式 | 命令行运行 | 双击启动 |
| 控制方式 | 命令行参数 | 托盘菜单 |
| 日志输出 | 控制台 | 文件 |
| 自启动 | 需手动配置服务 | 一键设置 |
| 窗口 | 控制台窗口 | 无窗口 |
