# macOS 原生标题栏方案失败记录

## 背景

为了让 macOS 版 `claude-code-启动器` 能够像原生 Mac 应用一样在左上角显示交通灯（关闭/最小化/最大化），同时保留应用自定义的左侧边栏切换按钮和配置/项目 Tab，曾尝试在设置中增加“标题栏风格”选项：

- **Windows 风格**：完全自定义标题栏，右侧显示最小化/最大化/关闭按钮。
- **macOS 风格**：显示系统原生交通灯，隐藏右侧自定义窗口控制按钮。

## 尝试方案

1. **Tauri 运行时切换**
   - 通过 `setDecorations(true)` + `setTitleBarStyle('overlay')` + `hiddenTitle` 在 macOS 上显示原生交通灯。
   - 通过 `transparent: true` + CSS `border-radius` 实现窗口圆角。

2. **伪最大化拦截**
   - macOS 进入系统全屏/最大化时会自动把交通灯移到菜单栏并隐藏。
   - 为保持交通灯始终可见，编写了 `src-tauri/src/macos_window_controls.rs`，用 `cocoa`/`objc` 注册自定义 `NSWindowDelegate`：
     - 拦截 `windowShouldZoom:toFrame:` 并返回 `NO`，阻止系统 zoom。
     - 改为手动把窗口 resize 到当前屏幕的 `frame`，实现“类全屏”效果。
     - 同时劫持绿色最大化按钮的 `target/action`，使其调用自定义 toggle。

3. **反复调试的问题**
   - `setDecorations(true)` 会重置 `NSWindow` 的 delegate，导致拦截器丢失，需要在前端切换风格后重新安装 delegate。
   - 需要开启 `macOSPrivateApi` 才能让 `transparent` + overlay 标题栏生效。
   - 屏幕坐标计算错误：曾把 `NSScreen` 的逻辑像素（points）再除以 `scale_factor`，导致最大化后窗口只有屏幕四分之一。
   - 使用 `visibleFrame` 时窗口会避开菜单栏，看起来像“普通窗口最大化”而非 macOS 全屏；改用 `frame` 后视觉效果接近全屏，但仍然无法达到稳定、可交付的体验。

## 失败结论

在 Tauri v2 + 无边框透明窗口 + overlay 标题栏的组合下，macOS 的原生交通灯与自定义标题栏/最大化行为存在较多边界问题：

- 需要依赖 private API（`macOSPrivateApi`）。
- 运行时切换 `decorations`/`titleBarStyle` 会重置原生对象状态，需要反复重新安装 Objective-C delegate。
- 自定义 pseudo-maximize 无法完全复刻系统全屏/缩放的行为，且在不同屏幕、DPI、多显示器下容易出坐标/尺寸偏差。
- 维护成本过高，对跨平台代码侵入性大。

因此决定**撤销 macOS 风格标题栏功能**，项目还原为统一的 Windows 风格自定义标题栏。

## 已撤销的改动

- 删除 `src-tauri/src/macos_window_controls.rs`。
- `src-tauri/src/lib.rs`：移除 `macos_window_controls` 模块、相关命令注册及 setup 中的 macOS 初始化。
- `src-tauri/src/persistent_state.rs`：移除 `load_title_bar_style` / `save_title_bar_style` 命令（保留 `title_bar_style` 字段用于兼容旧状态文件）。
- `src-tauri/Cargo.toml`：移除 `cocoa`、`objc` 依赖及 `macos-private-api` feature。
- `src-tauri/tauri.conf.json`：移除 `macOSPrivateApi`、`transparent`、`titleBarStyle`、`hiddenTitle`、`trafficLightPosition`。
- `src-tauri/capabilities/default.json`：移除 `core:window:allow-set-decorations`、`core:window:allow-set-title-bar-style`。
- `src/App.vue`：移除平台检测、`titleBarStyle` 状态、设置中的标题栏风格选项、macOS 专用伪最大化调用及左侧交通灯 padding。
- `src/assets/styles/theme.css`：移除 `#app` 的圆角与阴影样式。

## 后续方向

若未来仍要支持 macOS 原生标题栏，建议：

- 使用 Tauri 官方/community 提供的原生标题栏插件，而非手写 Objective-C delegate。
- 或直接在 macOS 上采用系统默认 `decorations: true`，接受系统标题栏样式，不再做高度定制。

当前保留 Windows 风格自定义标题栏，功能稳定且维护简单。
