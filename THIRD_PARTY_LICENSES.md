# 第三方组件

ZM-LINUX 本身使用 MIT 许可证。主要第三方组件包括：

- zmBox（https://gitee.com/duskeye/zmBox，MIT）：游戏宿主交互流程参考，核对版本 `bacc1d6f6adbc7634c82fb02e3bf41b5e5bedc30`。本项目使用独立 Rust 架构，不分发其 AIR 运行时或游戏资源。
- Ruffle（固定版本 `a4f5b5256e245693bc9077ef6c6b6abc95490e7f`）：MIT 或 Apache-2.0。`vendor/ruffle/core` 保留该版本源码及 JSON 数字精度修复，许可证见 `vendor/ruffle/LICENSE.md`。
- egui、eframe、wgpu、Tokio、reqwest、CPAL：依各自上游许可证使用，具体版本记录在 `Cargo.lock`。

游戏程序、名称、图像、音频、商标与在线服务归其各自权利人所有。ZM-LINUX 不在源码仓库或安装包中分发游戏 SWF 与在线资源。
