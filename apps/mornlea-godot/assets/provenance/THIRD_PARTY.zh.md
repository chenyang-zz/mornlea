# Godot 试点资源来源

本文件是仅桌面端 Godot 试点的初始资源与依赖白名单。未列出的组件一律禁止使用。新增或替换引擎二进制、绑定、字体、材质来源、着色器来源或插件时，必须先取得变更批准、补齐来源记录，并在分发前通过 `GodotResourceLicenses` 审计。

| Category | Component | Source | Version | License | Distribution obligations | Pilot authorization |
|---|---|---|---|---|---|---|
| Engine | Godot Engine | https://github.com/godotengine/godot | 4.7.2-stable | MIT | 保留 Godot 版权与 MIT 许可证文本，并随所选官方桌面端二进制分发其上游第三方声明。 | 仅批准用于已登记的 macOS 桌面端制品；Windows 与 Linux 桌面端使用前必须分别登记校验和。 |
| GDExtension binding | godot-rust | https://github.com/godot-rust/gdext | godot crate 0.5.5，启用 api-4-7；Cargo.lock 精确锁定 | MIT | 在源码以及包含该绑定的分发物中保留上游版权与 MIT 许可证文本，并维持精确 crate 版本和 Cargo.lock 解析结果。 | Rust 1.97.1 兼容性探针通过后允许用于桌面端试点；任何升级都必须重新探测并更新来源记录。 |
| Scripting runtime | Py4Godot | https://github.com/niklas2902/py4godot | 项目加固版 4.7-alpha21-mornlea.3，基于源修订 d8e17428deeb0428587349b663f6da26cd71ef3a；上游发布包 SHA-256 为 7fa28db6e5614523a4a9e092ca2e75fb76dfa1f4c385aaf6dd624eb33bf3eb4c；源码 SHA-256 为 e7a763fee661acecf1a4878501737299ba5fb7f6f0e58b4d3ac95fc58047fdd0；补丁序列 SHA-256 为 e26ce0953ad765df3e1d3c1c10886663ed3efa7626741d3b4718e1b7e63e207d；加载器 SHA-256 为 bb2e4f7fef9fb7bf10150e80892d91077ec34cd951d722bff220798604b8ef4f；原生主库 SHA-256 为 15558cb16f7dc65ad59a9bcdf101c142a6f4ace52d9fac0b819fa6e61388eaf6；物化制品 SHA-256 为 8da24f2f79d033a88493410930ced3d47161e1963f68b278da0ca07339c3c465 | MIT | 保留上游版权与 MIT 许可证文本；只能从钉定源码和有序项目补丁序列复现，使用精确的 Apple clang 21.0.0 与 macOS SDK 26.5 构建输入，以离线 Cython 3.3.0 重建原生主库并保留 NIL 空值，验证全部摘要，排除运行时安装工具；当上游版本独立通过同一矩阵时删除项目补丁。 | 仅允许用于通过离线隔离编辑器/无窗口/导出、项目桥共存、动态资源生命周期及 100 轮验证的 macOS Apple Silicon Python 主导试点。该衍生版本是可替换的脚本适配器，不暴露 Mornlea 玩法或数据 API；Windows 与 Linux 尚未验证。 |
| Interpreter | CPython runtime | https://github.com/python/cpython | 项目加固版 Py4Godot 4.7-alpha21-mornlea.3 内嵌的 macOS Apple Silicon CPython 3.14.4 | Python Software Foundation License Version 2 | 保留内嵌的 CPython LICENSE.txt 及适用上游声明；合格单元排除 pip 与 ensurepip，且不得解析系统、用户、当前工作目录或环境变量控制的 Python 路径。 | 仅允许作为精确验证过的加固单元的一部分使用。系统 Python、开发者虚拟环境、运行时安装工具与运行时下载仍是禁止的依赖。 |
| Font | Noto Sans CJK SC Regular | https://github.com/notofonts/noto-cjk | 源修订 f8d157532fbfaeda587e826d4cd5b21a49186f7c；字体 SHA-256 为 2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b | OFL-1.1 | 每份字体副本都必须保留 SIL 开放字体许可证与已核验的来源记录；Godot 项目内副本只能通过 `sync-assets.sh` 生成。 | 仅允许把仓库已登记字体用于通过生成清单验证的制品；替换字体必须单独进行来源审查。 |
| Material | Mornlea registered atlas | packages/client/assets | 生成清单中的 `input_revision` 与 `input_checksum` | Project-owned, MIT, and CC0-1.0 | 从权威注册表确定性生成 atlas；保留根许可证，以及由 `sync-assets.sh` 复制的 Pastelcraft 许可证、署名与逐文件来源记录；禁止加入 Mojang 或未登记的第三方美术资源。 | 仅在生成文件清单与校验和验证通过后允许使用。 |
| Shader | Mornlea project shaders | apps/mornlea-godot | 试点源码，未发布 | MIT (project-owned) | 新着色器源码必须在本仓库内原创并保留根 LICENSE；未来若借用任何片段，必须在使用前单独登记。 | 仅允许新编写的项目自有源码。 |
| Plugin | mornlea_bridge | packages/engine/crates/mornlea_godot | 试点源码，未发布 | MIT (project-owned) | 只能从已钉定的仓库工作区构建，保留根 LICENSE，并且只打包发布清单声明的目标桌面端库。 | 只允许这个项目自有插件；禁止应用市场或下载插件。 |

Godot 可执行文件、导出模板、godot-rust 绑定、Py4Godot 扩展与 CPython 解释器均为独立的上游作品，本仓库不主张其所有权。试点不授权移动端、Web、主机、应用市场、系统搜索路径或运行时下载的组件。
