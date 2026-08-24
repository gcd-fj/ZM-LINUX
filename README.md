# ZM-LINUX

ZM-LINUX 是由 gcd-fj 独立设计和实现的《造梦西游4/5》Linux 原生客户端。登录、资源管理、账号管理与游戏画面全部位于同一个 Rust + egui 窗口中，不依赖 Wine、Adobe AIR、Electron、浏览器壳或外部播放器进程。

游戏运行核心固定使用 Ruffle commit `a4f5b5256e245693bc9077ef6c6b6abc95490e7f`，并与 egui 共用同一个 wgpu Device、Queue 和 Adapter。游戏 SWF 与后续资源不会随程序分发，只在用户启动游戏时从官方地址获取。

## 当前能力

- 4399账号登录、验证码与游戏token请求。
- 造梦西游4/5版本发现、安全下载、校验与缓存回滚。
- 单窗口GPU纹理渲染，支持等比例缩放、键鼠、滚轮、文本输入与IME。
- Wayland优先，兼容X11；F11进入纯游戏全屏，F11或Esc退出全屏。
- 多账号选择与切换，密码优先保存到Linux Secret Service。
- 音量、缓存清理、脱敏诊断、AppImage桌面入口安装与卸载。

## 环境要求

- Ubuntu 24.04 x86_64（优先Wayland，自动回退X11）。
- Rust 1.95或更高版本（仅源码构建需要）。
- Linux Secret Service（GNOME Keyring或KWallet）。
- Vulkan兼容显卡驱动。

源码构建所需系统开发包：

```bash
sudo apt install build-essential pkg-config libasound2-dev libudev-dev libfontconfig-dev libdbus-1-dev
```

## 构建与运行

```bash
cargo test --workspace
cargo build --release --locked --bin zm-linux
./target/release/zm-linux
```

程序同时编译Wayland与X11后端。存在有效的`WAYLAND_DISPLAY`或`WAYLAND_SOCKET`时会使用原生Wayland窗口，否则回退X11。

应用数据遵循XDG目录：

- 配置：`$XDG_CONFIG_HOME/zm-linux/config.toml`
- 缓存：`$XDG_CACHE_HOME/zm-linux/`
- 日志：`$XDG_DATA_HOME/zm-linux/logs/`

配置文件不会保存密码、Cookie或游戏token。密钥环不可用时，密码只保留在当前进程内存中。

## AppImage

安装`linuxdeploy`后执行：

```bash
./packaging/appimage/build.sh
```

产物为`dist/ZM-LINUX-x86_64.AppImage`及对应SHA-256文件。AppImage只包含一个`zm-linux`程序，不下载或捆绑外部播放器。首次运行会幂等安装用户级desktop入口和hicolor图标；也可在设置页重新安装或卸载该入口。

## 使用说明

1. 启动后点击“切换账号”，选择已保存账号或“使用其他账号”。
2. 新账号模式输入4399账号和密码；保存账号模式仅在密钥环缺少密码时显示密码框。
3. 选择造梦西游4或5并登录。初始化页会依次显示4399认证、资源检查、创建播放器、注入会话。
4. 游戏顶部工具栏可调整音量、复制诊断、切换账号、全屏或退出游戏。全屏后只显示游戏画面。

## 故障诊断

- 图标未更新：在设置页点击“重新安装桌面入口”，随后重新打开应用。
- 密钥环不可用：确认GNOME Keyring或KWallet已启动；程序会允许本次手动输入密码。
- 游戏资源异常：退出游戏后在设置页清空缓存，重新启动会自动下载。
- 会话注入失败：程序会在20秒后停止播放器并返回明确错误，不会无限停留在“连接服务器中”。在设置页复制诊断信息用于排查。

诊断日志会遮蔽密码、Cookie、验证码和完整token。游戏内仅允许打开明确的4399官方HTTPS页面，本地文件与未授权协议会被拒绝。

## 许可与版权

ZM-LINUX 使用 MIT 许可证，作者为 gcd-fj。Ruffle与其他组件的说明见[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)。游戏程序、素材、商标及在线服务归其各自权利人所有。
