造梦西游 4 / 5 Linux 桌面客户端，使用 Rust、egui 和内嵌 Ruffle。

### 下载与运行

下载附件中的 `ZM-LINUX-x86_64.AppImage`，适用于 Linux x86_64。

```bash
chmod +x ZM-LINUX-x86_64.AppImage
./ZM-LINUX-x86_64.AppImage
```

若系统缺少 FUSE，可使用解包运行模式：

```bash
./ZM-LINUX-x86_64.AppImage --appimage-extract-and-run
```

将 `.AppImage` 和 `.sha256` 文件下载到同一目录后，可以校验文件完整性：

```bash
sha256sum --check ZM-LINUX-x86_64.AppImage.sha256
```

### 说明

- 支持游戏选择、4399 账号登录、资源下载、音量控制、全屏和运行诊断。
- 游戏资源在启动时从官方地址获取，不包含在安装包中。
- 游戏兼容性仍在完善中，会话注入成功不代表全部游戏功能均已验证。
