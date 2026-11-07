# P0 ①②③：两个 WKWebView、样式注入、自定义 scheme 字体、焦点与快捷键

`./build.sh` 只用 Command Line Tools（`swiftc` + 手搭 `.app` + `codesign -s -`），不需要 Xcode。

运行（仓库根目录）：
```
uv run marimo edit --headless --port 2719 --token-password spike --skip-update-check spikes/p0/1-webviews/nb.py &
SPIKE_PROBE=1 SPIKE_QUIT=1 SPIKE_OUT=/tmp spikes/p0/1-webviews/build/ArboriaP0.app/Contents/MacOS/ArboriaP0
```
`SPIKE_PROBE` 加载 8 秒后在两个 WebView 里跑探测 JS、存截图，再用 `NSApp.sendEvent` 合成按键（和真键盘走同一条分发路径：window → WKWebView → 主菜单）。

## 结论（2026-11-06，macOS 26.6.2，marimo 0.25.2）
| 项 | 结果 |
|---|---|
| marimo 在第二个 WKWebView | ✅ `http://127.0.0.1:2719/?access_token=…` → 303 设 cookie → notebook 正常（3 个 cell 都渲染、能运行）。顶层页面，不受 ITP 第三方 cookie 限制。**不需要 ATS 例外**（127.0.0.1 默认允许）。 |
| 样式注入 | ⚠️ **`WKUserStyleSheet` 不是公开 API**（只有私有的 `_WKUserStyleSheet`）。改用公开的 `WKUserScript`（`atDocumentStart`，所有 frame）往 head 插 `<link href="arboria://app/marimo.css">`，效果一样：正文、标题、CodeMirror 都换成我们的字体和底色。marimo 自己的字体规则选择器很具体（标题是 `Lora`），每块都要单独覆盖，选择器集中在 `marimo.css`。marimo 页面里没有 shadow DOM（`shadows: 0`），CSS 能到达所有地方。ADR 里的 `WKUserStyleSheet` 要改成这个做法。 |
| 自定义 scheme 字体 + CORS | ✅ `WKURLSchemeHandler` 注册在两个 WebView 的配置上；http 源的 marimo 页面加载 `arboria://fonts/*.woff2`，响应带 `Access-Control-Allow-Origin: *` 后 `document.fonts` 显示 loaded。每个 WebView 各读一次（Noto 7.5 MB），可接受。 |
| 剪贴板 | ✅ 有 Edit 菜单（cut/copy/paste/selectAll 发给 first responder）时，原生剪贴板 ⇄ 网页双向都通。**没有 Edit 菜单 ⌘C/⌘V 在 WKWebView 里不工作**，薄壳必须带。 |
| 快捷键 | 网页先拿到按键：页面 `preventDefault` 了的（marimo 的 ⌘K 命令面板、⌘S 保存）菜单就不触发；页面没处理的（⌘1）落到主菜单。我们的页面 A 不拦 ⌘K，所以 A 聚焦时菜单的 ⌘K 触发、页面也看到 keydown。 |
| 焦点 | `window.makeFirstResponder(webView)` 切换 WebView 焦点可靠；菜单 ⌘1/⌘2 做“回到侧栏 / 回到 notebook”。 |

### 对计划的修改
- “用 `WKUserStyleSheet` 注入”改为“`WKUserScript` 在 document start 插 `<link>` 指向 `arboria://app/marimo.css`”。
- app 全局快捷键要避开 marimo 的键（⌘K、⌘S、⌘Enter、⇧Enter 等，P1 从 marimo 的快捷键表列出来）；确实要抢的键在 window 的 `performKeyEquivalent` 里先截（P1 再验证），否则以页面为准。
- marimo 自带 AI 的入口（“Generate with AI”、侧栏的 AI 图标）还在，P2 用 CSS 隐藏并查 marimo 配置能否关掉。
