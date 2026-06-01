# CMD Runner v0.6.0

轻量级 Windows BAT/CMD 文件管理器，基于 Rust + egui 构建。

## 功能

- **拖拽添加** - 直接拖入 `.bat` / `.cmd` 文件到窗口
- **多标签管理** - 4列网格圆角卡片布局，每个标签独立运行
- **实时输出** - 控制台输出实时显示在下方深色面板
- **自定义命名** - 每个标签可自定义名称
- **配置持久化** - 标签配置自动保存到 `cmdrunner.toml`，支持导入/导出
- **系统托盘** - 最小化到系统托盘，右键恢复窗口
- **无弹窗** - 使用 `CREATE_NO_WINDOW` 隐藏 cmd.exe 控制台窗口

## 截图

| 标签卡 | 运行输出 |
|--------|----------|
| 4列网格圆角卡片 | 深色背景实时显示 |

## 安装

### 从源码编译

需要 [Rust](https://www.rust-lang.org/tools/install) 工具链。

```bash
git clone https://github.com/momokula123/cmdrunner.git
cd cmdrunner
cargo build --release
```

编译产物位于 `target/release/cmdrunner.exe`。

### 直接下载

从 [Releases](https://github.com/momokula123/cmdrunner/releases) 页面下载最新版本。

## 使用

1. 启动 `cmdrunner.exe`
2. 拖入 `.bat` / `.cmd` 文件，或点击「添加文件」按钮
3. 在标签卡上点击 ▶ 运行，点 ⏹ 停止
4. 点击标签卡切换查看对应输出
5. 双击标签名称可重命名
6. 点击 X 关闭标签（会弹出确认对话框）

### 配置文件

程序同目录下自动生成 `cmdrunner.toml`，保存所有标签的路径和自定义命名：

```toml
[[tabs]]
path = "C:\\scripts\\build.bat"
custom_name = "构建项目"

[[tabs]]
path = "C:\\scripts\\deploy.bat"
custom_name = "部署上线"
```

支持「导入配置」和「导出配置」功能，方便在不同机器间同步。

## 快捷操作

| 操作 | 说明 |
|------|------|
| 拖拽文件到窗口 | 添加 BAT 标签 |
| ▶ 按钮 | 运行当前 BAT |
| ⏹ 按钮 | 停止运行中的 BAT |
| X 按钮 | 关闭标签（需确认） |
| 双击标签名 | 重命名 |
| 最小化到托盘 | 隐藏到系统托盘区 |
| 全部停止 | 停止所有运行中的 BAT |
| 清空全部 | 删除所有标签 |

## 技术栈

- [Rust](https://www.rust-lang.org/)
- [egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) - 即时模式 GUI
- [tray-icon](https://github.com/nicokoch/tray-icon) - 系统托盘
- [toml](https://github.com/toml-rs/toml) - 配置序列化
- [rfd](https://github.com/nicholasbishop/rfd) - 原生文件对话框
- [windows-sys](https://github.com/microsoft/windows-rs) - Windows API 绑定

## 系统要求

- Windows 10/11
- 无额外运行时依赖

## License

MIT
