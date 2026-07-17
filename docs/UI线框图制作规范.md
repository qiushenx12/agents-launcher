# UI线框图制作规范

> 适用文件：`art/美术风格/UI线框图.html`
> 输入文档：`策划案/*.html`、系统功能策划文档、`art/美术风格/美术风格需求_正式版.md`
> 输出文件：`art/美术风格/UI线框图_*.png`、`art/美术风格/UI状态反馈_*.png`
> 目标用途：把策划案中的 UI 结构、界面状态和美术生图提示词整理到同一个 HTML 工作台中，再从该 HTML 截图输出 PNG。

---

## 1. 总体工作流

线框图制作采用“文档先行、HTML 制作、截图导出”的流程：

1. **先写策划案**：包括系统功能文档、界面流程说明、状态反馈规则、按钮/入口/弹窗/货币等 UI 需求。
2. **整理美术风格需求**：在 `美术风格需求_正式版.md` 中明确公共前提、风格描述、材质、颜色、负向要求和 ChatGPT image-2 使用方式。
3. **制作 `UI线框图.html`**：把策划案涉及到的常规 UI 界面、特殊状态、状态反馈、右侧 image-2 prompt 全部放进这个文件。
4. **浏览器检查页面**：确认每个 `.shot-card` 都能独立表达一个界面或状态，右侧 prompt 正确挂载，顶部美术风格说明可读。
5. **从 `UI线框图.html` 输出 PNG**：使用 Playwright 或浏览器截图，按 `.shot-card` 逐张导出 PNG。

`UI线框图.html` 是唯一线框源文件，不再单独维护状态反馈 HTML。PNG 是最终交付物，不是线框图制作源文件。后续修改界面或状态时，应回到 `UI线框图.html` 修改，再重新导出 PNG。

---

## 2. 制作目标

`UI线框图.html` 不是单纯的线框预览页，而是一个**线框图 + 状态反馈 + 生图提示词工作台**。

页面需要同时满足：

1. **策划确认**：能直观看到每个界面的布局、元素数量、状态和说明。
2. **状态表达**：所有状态反馈都要放在真实界面语境中，不做脱离界面的大集合图。
3. **美术沟通**：顶部集中说明正式美术风格和公共前提。
4. **生图使用**：每张线框图右侧放对应的 ChatGPT image-2 中英提示词。
5. **截图导出**：每个界面或状态都能作为独立 `.shot-card` 被截图输出为一张 PNG。

---

## 3. 页面整体结构

HTML 页面按以下顺序组织：

1. `header`
   - 页面标题。
   - 说明线框图来源、制作目的和输出用途。

2. `.art-brief`
   - 正式美术风格说明。
   - 公共前提、风格描述、材质组件、负向要求。
   - 内容来自 `美术风格需求_正式版.md` 的整理版。

3. `.gallery`
   - 所有线框图卡片。
   - 每个卡片使用 `.shot-card`。
   - 常规界面、弹窗、结算页、商城页、设置页、状态反馈都放在这里。
   - 左侧展示线框图和说明。
   - 右侧由脚本自动挂载对应 image-2 prompt。

4. `.prompt-library`
   - 隐藏的 prompt 数据源。
   - 不直接显示在页面底部。
   - 每个 `.prompt-card summary` 是 prompt 标题，`.prompt-text` 是 prompt 内容。

5. `attachPromptsToWireframes()` 脚本
   - 根据 `data-shot` 找到对应 prompt。
   - 自动创建 `.shot-prompt` 面板并插入到对应 `.shot-card`。

---

## 4. 卡片布局规范

每个界面或状态卡片使用以下结构：

```html
<section class="shot-card" data-shot="01-home">
  <div class="shot-head">
    <div class="shot-title">主界面</div>
    <div class="shot-code">UI-WF-01</div>
  </div>

  <!-- 左侧线框图，在 HTML 中直接绘制 -->
  <div class="phone">...</div>

  <!-- 左侧说明 -->
  <div class="notes">
    <b>说明</b>
    <ul>
      <li>...</li>
    </ul>
  </div>

  <!-- 右侧 prompt 由脚本自动生成，不手写 -->
</section>
```

布局要求：

- `.shot-card` 使用左右两列。
- 左侧固定展示线框图和说明。
- 右侧展示对应 prompt。
- 页面窄屏时改为上下排列。
- 不要再把所有 prompt 堆到页面底部给用户查找。
- 每个 `.shot-card` 应能独立截图，不依赖页面外的说明才能看懂。

---

## 5. 手机线框规范

常规界面和状态界面都应优先在 HTML 中直接绘制，基础容器使用 `.phone`：

```html
<div class="phone">
  <div class="notch"></div>
  <div class="scene">
    ...
  </div>
</div>
```

基础参数：

- 手机宽度：`360px`
- 手机高度：`780px`
- 比例约为 `9:19.5`
- 顶部刘海：`.notch`
- 顶部刘海安全区：`--safe-top: 56px`
- 底部上滑/返回手势安全区：`--safe-bottom: 28px`

线框视觉规则：

- 黑白结构线框为主。
- 使用圆角、虚线、浅灰辅助线表达状态。
- 可点击元素要避开顶部刘海区和底部上滑/返回手势区。
- 不在同一界面堆过多说明文字。
- 状态反馈可以使用浮层、遮罩、红点、禁用态、toast、飘字、按钮置灰等 HTML/CSS 元素表达。

底部安全区要求：

- 底部系统手势区不是游戏 UI 功能区，不能放置主按钮、Tab 图标、确认/取消按钮、购买按钮等核心可点击元素。
- 底部 Tab、底部按钮、力度条等 UI 应放在 `--safe-bottom` 之上，安全区高度以手机外框底部圆角影响范围为准，不要额外占用过多画面空间。
- 底部 Tab 可以整体向下贴近屏幕底部，但必须保留在 home indicator 横条上方；当前主界面样板中，激活 Tab 底部到横条顶部约 `21px`。
- 在线框图中只保留短横条模拟系统 home indicator，不在手机框内写“底部安全区”等说明文字。
- home indicator 横条放在底部安全区内并靠近手机底边；当前样板中，横条距手机外框底边约 `8px`。
- 不要用斜纹、底色块或虚线框强调底部安全区；安全区通过留白和横条表达即可。
- 若某个界面需要底部操作栏，操作栏本身仍属于游戏 UI，操作栏下方还需要保留 `--safe-bottom`。
- 截图给生图模型时，prompt 中应继续要求遵守顶部刘海安全区和底部手势安全区。

示例结构：

```html
<div class="scene">
  <div class="screen-body">...</div>
  <div class="footer-tabs">...</div>
  <div class="bottom-safe-zone">
    <span class="home-indicator"></span>
  </div>
</div>
```

推荐 CSS：

```css
:root {
  --safe-bottom: 28px;
}

.footer-tabs.tabs-lower {
  align-items: flex-end;
  padding-bottom: 4px;
}

.bottom-safe-zone {
  height: var(--safe-bottom);
  display: flex;
  align-items: center;
  justify-content: center;
  padding-top: 10px;
  flex: 0 0 var(--safe-bottom);
}

.home-indicator {
  width: 92px;
  height: 4px;
  border-radius: 999px;
  background: var(--line);
  opacity: .7;
}
```

---

## 6. 状态反馈制作规范

状态反馈不再使用“大集合图”。所有状态都应作为 `UI线框图.html` 中的独立 `.shot-card` 制作，然后从该 HTML 导出 PNG。

状态反馈卡片示例：

```html
<section class="shot-card" data-shot="state-01" data-prompt-title="STATE-01 主界面首次开始">
  <div class="shot-head">
    <div class="shot-title">状态反馈 · 主界面首次开始</div>
    <div class="shot-code">STATE-01</div>
  </div>

  <div class="phone state-home-start">
    <div class="notch"></div>
    <div class="scene">
      <!-- 在这里直接绘制主界面、首次开始按钮高亮、必要提示 -->
    </div>
  </div>

  <div class="notes">
    <b>状态说明</b>
    <ul>
      <li>触发：玩家首次进入主界面。</li>
      <li>反馈：主按钮显示“开始游戏”，无继续进度。</li>
    </ul>
  </div>
</section>
```

状态拆分规则：

- 状态图必须发生在真实界面语境中。
- 状态反馈必须直接写在 `UI线框图.html` 的 `.shot-card` 中，使用 `.phone` 和 HTML/CSS 绘制，不引用已导出的状态 PNG 作为源图。
- 每张状态图必须写清楚“触发”和“反馈”。
- 一个真实界面能自然容纳多个同类状态时，可以放在一张图，例如地图节点的未解锁、当前、已通关、宝箱、Boss 等节点状态。
- 一个状态会遮挡、改变或强调不同界面语境时，应拆成单独一张图，例如每日挑战未开放、广告失败、支付失败、金币不足、+3 飘字。
- 禁止把已经导出的 PNG 再作为状态线框源图回填到 `UI线框图.html`。

当前状态反馈建议输出为以下独立卡片，并最终导出为对应 PNG：

- `UI状态反馈_01_主界面首次开始.png`
- `UI状态反馈_02_主界面继续进度.png`
- `UI状态反馈_03_每日挑战未开放.png`
- `UI状态反馈_04_每日挑战今日已完成.png`
- `UI状态反馈_05_底部Tab红点.png`
- `UI状态反馈_06_地图节点状态.png`
- `UI状态反馈_07_游戏内加3道具飘字.png`
- `UI状态反馈_08_商城每日精选已领取.png`
- `UI状态反馈_09_商城广告失败.png`
- `UI状态反馈_10_商城支付失败.png`
- `UI状态反馈_11_泡泡卡片状态.png`
- `UI状态反馈_12_泡泡金币不足.png`
- `UI状态反馈_13_泡泡广告失败.png`
- `UI状态反馈_14_泡泡广告解锁成功.png`
- `UI状态反馈_15_设置开关与语言.png`

---

## 7. Prompt 挂载规范

右侧 prompt 不手写在每个卡片里，而是由隐藏的 `.prompt-library` + 脚本自动挂载。

### 7.1 prompt 数据源

```html
<section class="prompt-library" aria-label="image-2 生图提示词">
  <article class="prompt-card">
    <details>
      <summary>UI-WF-01 主界面</summary>
      <pre class="prompt-text">中文：...</pre>
    </details>
  </article>
</section>
```

`.prompt-library` 当前通过 CSS 隐藏：

```css
.prompt-library {
  display: none;
}
```

这样可以避免页面底部重复堆一份 prompt，但仍保留结构化数据源。

### 7.2 data-shot 映射

脚本中的 `promptMap` 负责把卡片绑定到 prompt：

```js
const promptMap = {
  '01-home': 'UI-WF-01 主界面',
  '02-level-map': 'UI-WF-02 关卡选择 · 章节地图',
  '03-game-hud': 'UI-WF-03 游戏内 HUD',
  '04-result-win': 'UI-WF-04 / UI-WF-05 结算',
  '05-result-fail': 'UI-WF-04 / UI-WF-05 结算',
  '06-shop': 'UI-WF-06 商城',
  '07-bubbles': 'UI-WF-07 泡泡收藏册',
  '08-settings': 'UI-WF-08 设置弹窗',
  'state-01': 'UI-WF-09 状态反馈'
};
```

所有 `state-01` 至 `state-15` 可以复用 `UI-WF-09 状态反馈` prompt。

如果需要让右侧标题显示具体状态名，在卡片上加：

```html
data-prompt-title="STATE-01 主界面首次开始"
```

脚本会显示：

```text
STATE-01 主界面首次开始 · image-2 Prompt
```

并在 prompt 内容开头追加当前状态场景。

---

## 8. 顶部美术风格说明规范

`.art-brief` 的内容来自 `美术风格需求_正式版.md`，但不需要全文复制。

必须包含：

- 公共前提。
- 风格描述。
- 关键词。
- 推荐色彩。
- 材质与组件。
- 负向要求。

文字要求：

- 正文字号不能过小。
- 正文颜色不要使用过浅灰色，应使用接近 `#4b5563` 的可读颜色。
- 关键词和色值使用 tag/pill 形式，便于扫读。

---

## 9. Prompt 内容规范

每个 prompt 必须适配“上传线框图后使用”的场景。

必须写清楚：

- 严格遵守线框图布局。
- 不新增按钮、入口、货币栏、弹窗、活动 banner。
- 使用 Pastel Bubble 粉彩泡泡风格。
- 说明该界面的重点美术要求。
- 避免乱码、错误元素数量、写实台球桌、重运营风格。

建议同时提供：

- 中文 prompt。
- English prompt。

如果模型输出文字错误，可追加：

```text
请保持所有 UI 文案短小清晰，不要生成乱码；若无法准确生成文字，请保留文本区域和按钮结构。
```

---

## 10. PNG 导出规范

所有 PNG 都应从 `UI线框图.html` 中按 `.shot-card` 截图导出。导出的 PNG 是结果文件，不再作为 HTML 线框制作的输入。

示例逻辑：

```python
from pathlib import Path
from playwright.sync_api import sync_playwright

out_dir = Path("D:/Work/1-泡泡台球/art/美术风格")
html_path = out_dir / "UI线框图.html"

exports = {
    "01-home": "UI线框图_01_主界面.png",
    "state-01": "UI状态反馈_01_主界面首次开始.png",
}

with sync_playwright() as p:
    browser = p.chromium.launch()
    page = browser.new_page(viewport={ "width": 1500, "height": 1200 }, device_scale_factor=2)
    page.goto(html_path.resolve().as_uri(), wait_until="networkidle")

    for shot_id, filename in exports.items():
        page.locator(f'[data-shot="{shot_id}"]').screenshot(path=str(out_dir / filename))

    browser.close()
```

注意：

- 截图前等待页面加载完成。
- 如果使用脚本自动挂载 prompt，截图时必须让浏览器执行 JS。
- 状态反馈 PNG 也从 `UI线框图.html` 的对应 `state-*` 卡片导出。
- 中文路径在 Windows 管道脚本中可能出现编码问题，必要时用 Unicode 转义路径。
- 临时预览图不要留在交付目录中。

---

## 11. 维护流程

新增界面或状态时按以下步骤：

1. 先确认策划案中对应功能、状态、入口、触发条件和反馈规则。
2. 若涉及美术风格变化，先更新 `美术风格需求_正式版.md`。
3. 在 `UI线框图.html` 的 `.gallery` 中新增一个 `.shot-card`。
4. 设置唯一 `data-shot`。
5. 填写 `.shot-title` 和 `.shot-code`。
6. 左侧使用 HTML/CSS 直接绘制 `.phone` 线框和状态元素。
7. 在 `.notes` 中写清楚说明、触发和反馈。
8. 在 `.prompt-library` 中新增或复用 prompt。
9. 在 `promptMap` 中增加映射。
10. 用浏览器检查：
    - `.shot-card` 数量是否正确。
    - `.shot-prompt` 是否全部挂载。
    - 状态反馈是否在真实界面语境中。
    - 页面是否没有旧稿或重复大集合图。
11. 从 `UI线框图.html` 批量截图导出 PNG。

---

## 12. 验收清单

交付前检查：

- [ ] 策划案中的主要 UI 界面已经覆盖。
- [ ] 策划案中的状态反馈已经覆盖。
- [ ] 顶部美术风格说明可读，不是浅灰小字。
- [ ] 每个线框卡片都是左图右 prompt。
- [ ] 没有底部堆叠的大段 prompt 影响查找。
- [ ] 常规界面和状态反馈都在 `UI线框图.html` 同一页面内制作。
- [ ] 手机线框同时考虑顶部刘海安全区和底部上滑/返回手势安全区。
- [ ] 底部 Tab 或底部操作栏位于 home indicator 横条上方，横条贴近底部圆角位置。
- [ ] 状态反馈不是大集合图，而是按真实界面语境拆分为独立卡片。
- [ ] 没有把导出的 PNG 回填为状态线框源图。
- [ ] 每张图都有说明。
- [ ] 每张图右侧都有对应 image-2 prompt。
- [ ] prompt 明确要求遵守线框图，不新增 UI 元素。
- [ ] 所有交付 PNG 都从 `UI线框图.html` 导出。
- [ ] 页面在 1500px 左右宽度下阅读舒适，在窄屏下能上下排列。
