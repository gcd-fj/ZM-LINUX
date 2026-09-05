# ZM-LINUX

使用 Rust 编写的造梦西游 4 / 5 桌面客户端，采用 egui + 内嵌 Ruffle。游戏资源在启动时从官方地址获取，不随程序分发。

项目参考 [zmBox](https://gitee.com/duskeye/zmBox) 的游戏宿主交互流程，重新设计 Rust 应用结构。目标平台为 Linux 和 Windows；Windows 验证由新增 CI 承担，实际游戏兼容性仍需逐项测试。

## 重构后的结构

| 模块 | 职责 |
| --- | --- |
| zm-app | 游戏库首页、账号界面、窗口与事件接入 |
| zm-launcher | 可取消的启动工作流、状态机、会话隔离 |
| zm-core | 公共模型、错误、两款游戏的配置 |
| zm-storage | 配置文件、内存凭据、系统密钥环、平台目录 |
| zm-auth | 4399 登录、验证码与令牌协议 |
| zm-assets | 版本发现、下载、完整性校验、缓存发布、SWF 桥接补丁 |
| zm-player | Ruffle 宿主、共享 GPU 渲染、输入、音频与诊断 |

完整设计、数据流及边界见 [架构说明](docs/ARCHITECTURE.md)。Ruffle 固定于 `a4f5b5256e245693bc9077ef6c6b6abc95490e7f`，与 egui 使用匹配的 wgpu 版本。

## 构建运行

需要 Rust 1.95 或更高版本。Ubuntu 开发依赖：

```bash
sudo apt install build-essential pkg-config libasound2-dev libudev-dev libfontconfig-dev libdbus-1-dev fonts-noto-cjk
cargo run --locked --bin zm-linux
```

调试版适合复现功能问题，性能评估请使用优化版：

```bash
cargo build --release --locked --bin zm-linux
./target/release/zm-linux
```

Windows 使用 MSVC Rust 工具链及 Visual Studio C++ 构建工具，执行同样的 Cargo 命令，程序为 `target/release/zm-linux.exe`。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## 使用

1. 在游戏库中选择造四或造五，填写或选择 4399 账号。
2. 登录需要验证码时填写图片内容；图片失败可单独刷新。准备阶段可取消启动。
3. 游戏工具栏支持音量、诊断、账号切换、全屏及退出。F11 切换全屏，Esc 退出全屏。
4. 设置中可清理资源缓存、查看上次游戏诊断。Linux 另提供桌面入口安装和卸载。

“宿主就绪”与“会话已注入”是不同阶段。会话注入成功不代表全部游戏功能已验证，游戏后续仍可能加载资源或遇到播放器兼容问题。游戏启动连续 90 秒没有资源完成或初始化进展才会停止播放器并保留诊断；不按总下载时间判定超时。

## 数据与缓存

Linux 遵循 XDG：配置在 `~/.config/zm-linux`，缓存在 `~/.cache/zm-linux`，数据与日志在 `~/.local/share/zm-linux`。Windows 使用系统配置、缓存及本地数据目录。

配置仅保存账号元数据与设置。密码保存在系统密钥环或当前进程内存，Cookie 和 token 仅用于当前会话。配置损坏时保留原文件并禁止本次运行覆盖，需备份修复后重新启动。

主 SWF 按内容哈希发布，清单最后切换。更新失败时可以使用校验通过且匹配当前桥接版本的旧缓存；补丁更新会触发重新下载。运行时资源按版本隔离、合并相同请求。清理资源不会删除新架构数据目录中的游戏 SharedObject，后者按游戏和 UID 分开。

## 桥接开发

仓库包含桥接源文件与编译后的 ABC。修改源文件后必须重新生成 ABC：

```bash
RUFFLE_ASC_JAR=/path/to/pinned-ruffle/tools/asc/asc.jar \
RUFFLE_PLAYERGLOBAL=/path/to/target/debug/build/ruffle_core-.../out/playerglobal_import.abc \
bash tools/build-bridges.sh
```

需要 Java，Ruffle 的 playerglobal 文件由正常 Cargo 构建产生。不要混用另一版本的编译输入。

## AppImage

安装 `linuxdeploy` 后执行 `./packaging/appimage/build.sh`。产物位于 `dist/`，包含程序及校验文件，不捆绑游戏资源。

## 许可

项目使用 MIT，第三方来源见 [第三方说明](THIRD_PARTY_LICENSES.md)。游戏程序、资源、商标及服务属于各自权利人。
