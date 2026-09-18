import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * dsh 配置面板从「一张卡片」变成了「左侧供应商栏 + 右侧内容」，于是这些断言要
 * 分成两处看：
 *
 * - `SHELL`（DshConfigPanel.vue）—— 外壳：供应商清单、页脚的「启动设置 / 设置」、
 *   以及右侧内容的切换。
 * - `STARTUP`（DshStartupSettingsPane.vue）—— 原来的整张卡片：访问范围、端口、
 *   版本、运行状态、端口清理、访问地址清单、进入 dsh 的跳转按钮。
 */
const SHELL = 'src/components/dsh/DshConfigPanel.vue'
const STARTUP = 'src/components/dsh/DshStartupSettingsPane.vue'
const EDITOR = 'src/components/dsh/DshProviderEditor.vue'

function scopedStyle(relativePath: string): string {
  const source = readFileSync(resolve(repoRoot, relativePath), 'utf8')
  const styleBlock = /<style\s+scoped>([\s\S]*?)<\/style>/.exec(source)
  assert.ok(styleBlock, `${relativePath} has no scoped style block`)
  return styleBlock[1].replace(/\/\*[\s\S]*?\*\//g, '')
}

function declarations(css: string, selector: string): Map<string, string> {
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const rule = new RegExp(`${escapedSelector}\\s*\\{([^}]*)\\}`).exec(css)
  assert.ok(rule, `missing rule for ${selector}`)

  const result = new Map<string, string>()
  for (const declaration of rule[1].split(';')) {
    const separator = declaration.indexOf(':')
    if (separator === -1) continue
    result.set(declaration.slice(0, separator).trim(), declaration.slice(separator + 1).trim())
  }
  return result
}

test('dsh access and port labels align to the left edge of their shared column', () => {
  const css = scopedStyle(STARTUP)
  const label = declarations(css, '.field-label')

  assert.equal(label.get('width'), '110px')
  assert.equal(label.get('text-align'), 'left')
})

/**
 * The port row used to be a full-width input with a hint line underneath and
 * 「重新检测端口」 at the hint's far end. The row is now compact: a shortened
 * input, the recheck button pinned to the row's right edge, and no hint text —
 * an occupied or invalid port still surfaces through the portError banner and
 * the 一键清理占用 block.
 */
test('the port row pairs a shortened input with the recheck button and drops the hint line', () => {
  const markup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')
  const row = /<div class="field-row">\s*<label class="field-label" for="dsh-port-input">端口<\/label>([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(row, 'missing the port field row')

  // The recheck button lives on the same row, after the input.
  assert.match(row[1], /id="dsh-port-input"[\s\S]*?@click="recheckPort"/)
  assert.match(row[1], /重新检测端口/)

  // The input keeps a fixed small width instead of filling the row, and the
  // button is pinned to the row's right edge.
  const css = scopedStyle(STARTUP)
  const input = declarations(css, '.field-row > .input--port')
  assert.equal(input.get('flex'), '0 0 auto')
  assert.ok(input.get('width') !== undefined, 'the port input must be shortened to a fixed width')
  assert.equal(declarations(css, '.field-row__port-action').get('margin-left'), 'auto')

  // The hint text line under the row is gone for good.
  assert.doesNotMatch(markup, /field-help--row|portHelp/)
})

test('the dsh action row holds only the service controls', () => {
  const markup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')
  const actions = /<div class="action-row">([\s\S]*?)<\/div>/.exec(markup)
  assert.ok(actions, 'missing dsh action row')

  // 复制链接 / 打开网页 / 二维码 used to live here and handed out a single
  // guessed address. They are per-address row actions now, so the action row
  // must not grow a second way to reach the service.
  assert.match(actions[1], /保存设置|设置已保存/)
  assert.match(actions[1], /@click="store\.start\(\)"/)
  assert.match(actions[1], /@click="store\.stop\(\)"/)
  assert.doesNotMatch(actions[1], /复制链接|打开网页|二维码/)
})

/**
 * 设置入口的位置变了：dsh 现在有自己的侧边栏，所以那个入口从卡片右下角搬到了
 * 侧边栏页脚——与其它三个前端的左下角一致。页脚里「启动设置」在「设置」**上方**，
 * 两个入口同属一层、同一尺寸；`.settings-entry` 的契约必须原样保留（App.vue 的
 * 点击空白关闭靠这个类名识别触发按钮，外观也要与别处一致）。
 *
 * 页脚上方那句版本提示已删除：它既不是这两个入口的说明，也不是操作所必需的信息，
 * 版本号改由右侧「写入说明」承载。
 */
test('the dsh sidebar footer stacks 启动设置 above the shared settings entry', () => {
  const markup = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  const footer = /<footer class="provider-sidebar__footer">([\s\S]*?)<\/footer>/.exec(markup)
  assert.ok(footer, 'missing the dsh sidebar footer')

  // 顺序：启动设置 → 设置，页脚里没有别的行。
  assert.ok(
    footer[1].indexOf('class="sidebar-entry"') < footer[1].indexOf('class="settings-entry"'),
    '启动设置 要在 设置 上方',
  )

  // 两个入口的标记形状一致：前置字形 + `<span>` 包住的标签。字形本身不钉死（它只是
  // 装饰），钉的是「两个入口同形」——「启动设置」早先只有光秃秃的文字，比同一层的
  // 「设置」少一个图标。
  assert.deepEqual(
    (footer[1].match(/[^\s<]+ <span>[^<]+<\/span>/g) ?? []).map((html) => html.replace(/^[^\s<]+ /, '')),
    ['<span>启动设置</span>', '<span>设置</span>'],
  )
  assert.match(footer[1], /@click="selectStartup"/)
  assert.match(footer[1], /@click="toggleSettings\(\$event\)"/)
  assert.doesNotMatch(footer[1], /version-note/, 'no leftover note above the 启动设置 entry')
  assert.doesNotMatch(markup, /version-note/)

  // 「启动设置」曾经只有 12px 字号，比同层的「设置」小一号——两个入口同属一层，
  // 尺寸必须与全局 .settings-entry（13px / 7px 8px）一致；图标与标签的间距也用
  // 同一个 flex + 8px gap，否则两个入口的字形落点会差几个像素。
  const css = scopedStyle(SHELL)
  const entry = declarations(css, '.sidebar-entry')
  assert.equal(entry.get('font-size'), 'var(--font-size-base)')
  assert.equal(entry.get('padding'), '7px 8px')
  assert.equal(entry.get('display'), 'flex')
  assert.equal(entry.get('gap'), '8px')
  assert.equal(entry.get('align-items'), 'center')
  assert.ok(!/version-note/.test(css), 'the version-note rule must be gone too')

  // 同一个全局浮层、同一个触发契约。
  assert.match(markup, /import \{ useSettingsPopover \} from '@\/composables\/useSettingsPopover'/)
  assert.match(markup, /const \{ toggleSettings \} = useSettingsPopover\(\)/)

  // 卡片里不再重复一个设置入口：那个位置现在只放跳转按钮。
  const startup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')
  assert.doesNotMatch(startup, /toggleSettings/, 'the settings entry must live in the sidebar footer only')
})

/**
 * 面板的说明文字收敛过一次：字段标题旁、按钮旁的散文一律删掉，只留「不写用户
 * 就做不对」的那几句。这条断言把删掉的那几处钉住，避免它们慢慢长回来：
 *
 * - 启动设置：标题下那句「通过 npx 运行 dsh web…」、访问范围与版本行下方的
 *   help 行（`accessHelp` / `versionHelp`）、地址清单那句两行长的令牌说明。
 * - 供应商编辑器：头部的「该供应商已存在于 settings.yaml…」、模型标题下的
 *   「手工声明的模型默认按纯文本对待…」、模型输入框下的「启动器不去联网查询…」、
 *   操作行末尾的「只操作当前供应商…」。
 *
 * 「写入说明」也按同一把尺子重写过一遍：原文是「只改动 … 里当前这一个供应商块，
 * 文件其余部分与注释原样保留」——没有主语、用「块」这种实现术语、还把「被改动的
 * 字段会重排并丢掉注释」这个真正的代价藏了起来。现在每句一个主语、一件可执行的事。
 * 标题也去掉了「写入」二字：这张卡片讲的已经不止写入（第一条就是左侧清单只列什么），
 * 「说明」是它真正的范围。
 *
 * 保留的两句都是「不写就会做错」的：留空的条件、端口占用清理的后果。密钥那句
 * 已删——令牌管理进了编辑器（认证令牌栏），说明卡不再给指路。
 */
test('the dsh config panels keep only the load-bearing help lines', () => {
  const startup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')
  assert.doesNotMatch(startup, /accessHelp|versionHelp/)
  assert.doesNotMatch(startup, /通过 npx 运行 dsh web/)
  assert.doesNotMatch(startup, /class="field-help"/)

  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  // 只留「不写就会做错」的那半句：留空的含义已经由下拉框的第一个选项说了。
  assert.match(editor, /手工声明的网关必须填。/)
  assert.doesNotMatch(editor, /目录里已有的供应商可留空/)
  assert.doesNotMatch(editor, /model-add-help|该供应商已存在于 settings\.yaml|只操作当前供应商/)

  // 说明卡片：标题是「说明」（不再只是写入的事），第一条讲左侧只列什么，句子要有
  // 主语（「写入只改…」），两件实事都要在——改的是哪一个、备份到哪。密钥的指路句
  // 已删（令牌管理进了编辑器）；实现术语与「原样保留」这种没参照物的说法不许回来。
  const shell = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  const note = /<section class="card source-note">([\s\S]*?)<\/section>/.exec(shell)
  assert.ok(note, 'missing the dsh write note card')
  assert.match(note[1], /<div class="card-title">说明<\/div>/)
  assert.match(note[1], /<p>只显示自定义供应商。<\/p>/)
  assert.match(note[1], /写入只改你正在编辑的这一个供应商/)
  assert.match(note[1], /settings\.yaml\.bak/)
  assert.match(note[1], /被改动的那个字段会按规范格式重排，它内部的注释不会保留/)
  assert.doesNotMatch(note[1], /密钥不在这个文件里/, '令牌管理进了编辑器，说明卡不再指路')
  assert.doesNotMatch(note[1], /设置 → 模型/)
  assert.doesNotMatch(note[1], /供应商块/, '实现术语「块」不许回到界面')
  assert.doesNotMatch(note[1], /密钥由 dsh 自己保管/, '密钥那句要给动作，不是给状态')
})

/**
 * 新增的主体：左侧是供应商清单（可增删、可拖拽排序），右侧是选中项的配置。
 * 清单项显示「显示名 / id · N 个模型」与写入状态，与 OpenCode 的模式一致。
 *
 * 清单列的是 `store.visibleProviders` 而不是全部供应商——另一条测试解释为什么。
 */
test('the dsh sidebar lists providers and the editor edits one of them', () => {
  const markup = readFileSync(resolve(repoRoot, SHELL), 'utf8')

  assert.match(markup, /v-for="\(item, index\) in store\.visibleProviders"/)
  assert.match(markup, /\{\{ item\.displayName \|\| item\.id \}\}/)
  assert.match(markup, /\{\{ item\.id \}\} · \{\{ item\.models\.length \}\} 个模型/)
  assert.match(markup, /@click="createProvider"/)
  assert.match(markup, /新建供应商/)
  // 未写入 / 待更新 / 已写入 三态。
  assert.match(markup, /'未写入'[\s\S]*?'待更新' : '已写入'/)

  // 两个右侧面板：供应商编辑器与启动设置。
  assert.match(markup, /<DshProviderEditor\s*\/>/)
  assert.match(markup, /<DshStartupSettingsPane v-show="pane === 'startup'"/)

  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  // 模型增删。
  assert.match(editor, /@click="addModel"/)
  assert.match(editor, /添加模型/)
  assert.match(editor, /@click="store\.removeModel\(provider, model\.id\)"/)
  // 供应商增删：删除要弹确认，并且说明不会碰到凭据文件。
  assert.match(editor, /await confirm\(/)
  assert.match(editor, /@click="removeProvider"/)
  assert.match(editor, /本次删除不会动它/)
})

/**
 * 操作反馈是 3 秒飘字；要用户处理的消息才常驻横幅（任务 202609181728080000 及其
 * 验收意见）。验收时的现象：「已写入供应商 … 已备份为 …」仍常驻顶部——当时只把
 * 「新增草稿」一条挪进了外壳的局部 toast，其余 setStatus 全走 banner。定稿后的
 * 分流在 store 的 setStatus 里：success / info → 飘字（3 秒消失），
 * warning / error → 顶部横幅常驻。两个例外：
 * - 「尚未创建 dsh 设置文件」是**空状态指引**，常驻（直接写 status，不过分流）；
 * - 凭据读取失败显示在令牌按钮行（字段级），本来就不走状态条。
 */
test('operation feedback is a three-second toast while errors stay in the banner', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')
  // 分流在 setStatus 里：success/info 进 toast 通道并清掉横幅，warning/error 留横幅。
  assert.match(store, /const toast = ref<string \| null>\(null\)/)
  assert.match(store, /const toastSeq = ref\(0\)/)
  const setStatus = /function setStatus\(tone: DshModelsStatus\['tone'\], message: string\) \{([\s\S]*?)\n  \}/.exec(store)
  assert.ok(setStatus, 'missing setStatus')
  assert.match(setStatus[1], /tone === 'success' \|\| tone === 'info'/)
  assert.match(setStatus[1], /status\.value = null/)
  assert.match(setStatus[1], /showToast\(message\)/)
  assert.match(setStatus[1], /status\.value = \{ tone, message \}/)
  // 空文件指引常驻：直接写 status，不过分流。
  assert.match(store, /status\.value = \{[\s\S]*?尚未创建 dsh 设置文件/)

  const shell = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  assert.match(shell, /class="dsh-config-panel__toast"/)
  // 面板 watch toastSeq 重置计时器：连发两条各自计满 3 秒。
  assert.match(shell, /watch\([\s\S]*?store\.toastSeq/)
  assert.match(shell, /window\.clearTimeout\(toastTimer\)/)
  assert.match(shell, /window\.setTimeout\([\s\S]*?3000\)/)
  // 新增草稿的反馈同样走飘字通道。
  assert.match(shell, /setStatus\('info', `已新增供应商草稿「\$\{draft\.id\}」；填好字段后写入设置文件。`\)/)
  // 横幅仍在，只显示 store.status（即 warning/error 与常驻空态）。
  assert.match(shell, /<ConfigStatusBanner[\s\S]*?v-if="store\.status"/)
})

/**
 * 左侧清单只列**自定义**路由——与 dsh 自己设置页上那枚「自定义」标签同一个判据：
 * pi-ai 内置目录在该路由键下不提供任何内容（上游 `entry.declared === true`）。
 * 反过来，`kimi-coding` 这种「自带目录 + 一把密钥」的条目在这里没有可编辑的内容，
 * 列出来只会让人以为漏了什么。
 *
 * 判据不在前端猜：后端读**已安装**的 pi-ai 目录清单（profile 的 node_modules）
 * 算好 `custom` 一起送过来，所以两处对同一条路由的判断必然一致，也不会随 pi-ai
 * 升级而漂移。清单读不到时按自定义处理——宁可多列，也不把用户自己声明的路由
 * 藏起来（藏起来的东西在这个界面里既看不见也改不了）。
 *
 * 这条断言守三件事：过滤本身、过滤不会丢数据（拖拽排序按位置原地替换，目录路由
 * 留在原位）、以及清单空着时界面必须解释一句。
 */
test('the dsh sidebar lists only custom routes', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')
  assert.match(store, /const visibleProviders = computed\(\(\) => drafts\.filter\(\(draft\) => draft\.custom\)\)/)
  // 新草稿按自定义处理：它是用户自己声明的，没有目录可依赖。
  assert.match(store, /自定义路由：用户自己声明的，没有目录可依赖[\s\S]*?custom: true/)
  // 选中项不能停在看不见的那一条上；且选中按对象身份跟踪，不按可变的路由键字符串
  // （2026-09-18 任务 202609181729240000：改 ID 撞名时 selectedId 找不到草稿）。
  assert.match(store, /function syncSelection/)
  assert.match(store, /selectedDraft\.value = visibleProviders\.value\[0\] \?\? null/)
  // 拖拽要按位置原地替换，不能只把可见项拼回列表（那会把目录路由挤掉）。
  assert.match(store, /function reorderVisible/)
  assert.match(store, /draft\.custom \? index : -1/)

  const markup = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  assert.match(markup, /store\.visibleProviders\.map\(\(item\) => item\.id\)/)
  assert.match(markup, /store\.reorderVisible\(newOrder\)/)
  // 文件里有供应商、却一条自定义的都没有时，空状态要说清它们去哪了。
  assert.match(markup, /本页只显示自定义供应商；settings\.yaml 里剩下的都是 dsh 自带的目录路由。/)

  const types = readFileSync(resolve(repoRoot, 'src/types/config.ts'), 'utf8')
  assert.match(types, /custom: boolean/)

  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/dsh_settings.rs'), 'utf8')
  assert.match(rust, /pub custom: bool/)
  assert.match(rust, /fn installed_catalog_routes/)
  assert.match(rust, /fn mark_custom_routes/)
  assert.match(rust, /@earendil-works/)
  assert.match(rust, /\.manifest\.json/)
  assert.match(rust, /routes\.contains\(id\)[\s\S]*?None => true/)
})

/**
 * 回归（任务 202609181729240000）：修改供应商 ID 时「当前选择」不得丢失。
 *
 * 供应商的 id 就是 settings.yaml 里的路由键，用户随时能改，也可能与另一条
 * 已存在的供应商撞车。曾经用字符串 `selectedId` 记选中，撞名时 `syncSelection`
 * 会在列表里找到**另一条**同 id 的供应商、误以为选中还在，编辑器瞬间切走。
 *
 * 修法：用对象身份（`selectedDraft`）而不是 id 字符串来跟踪选中。id 会变、
 * 会撞车，对象不会。`syncSelection` 按 `visibleProviders.includes(draft)` 判断，
 * 找不到时才兜底到第一条。
 */
test('selection tracks the draft object, not its mutable route id', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')

  // 选中状态是对象引用，不是字符串 id。
  assert.match(store, /const selectedDraft = ref<DshProviderDraft \| null>\(null\)/)
  assert.match(store, /const selectedProvider = computed\(\(\) => selectedDraft\.value\)/)

  // syncSelection 按对象身份判断，不查 id 字符串。
  assert.match(store, /visibleProviders\.value\.includes\(current\)/)
  assert.match(store, /selectedDraft\.value = visibleProviders\.value\[0\] \?\? null/)

  // 旧的字符串 id 写法不得再出现。
  assert.doesNotMatch(store, /const selectedId = ref/)
  assert.doesNotMatch(store, /drafts\.find\(\(item\) => item\.id === selectedId\.value\)/)

  // 外壳与编辑器都按对象身份取选中项。
  const shell = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  assert.match(shell, /store\.selectedProvider === item/)
  assert.match(shell, /store\.selectProvider\(item\)/)
  assert.doesNotMatch(shell, /store\.selectedId/)

  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  assert.match(editor, /const provider = computed\(\(\) => store\.selectedProvider\)/)
})

/**
 * 进入 dsh 配置页时落在**启动设置**上，不是供应商编辑器。
 *
 * dsh 与其它三个前端的形状不同：那三个只有一屏配置内容，进来落在编辑器上是对的；
 * dsh 这一页进来第一件要做的事通常是把服务启起来，供应商是启完才用得上的东西。
 * 所以首屏不按「有没有供应商」挑（那是曾经的写法），而是固定这一屏。
 *
 * 「首次」只指本次运行的第一次：面板是 v-show 常驻的（启动设置有跑着的计时器与
 * 端口探测，不能随切走销毁），首屏之后切走再切回，显示的是用户上一次选的那一屏，
 * 不再强制回位（2026-09-18 用户定稿——曾经的 activeKind watch 回位写法已删）。
 */
test('the dsh panel opens on the startup settings, not on the provider editor', () => {
  const markup = readFileSync(resolve(repoRoot, SHELL), 'utf8')

  assert.match(markup, /const pane = ref<DshPane>\('startup'\)/)
  // 曾经的写法：有自定义供应商就先画编辑器。
  assert.doesNotMatch(markup, /pane\.value = store\.visibleProviders\.length > 0/)
  // 首屏之后不回位：切为当前 CLI 不得重置用户的选择。曾经是 watch(activeKind)
  // 强制回「启动设置」，与「后续跟随当前选择」冲突，已删。
  assert.doesNotMatch(
    markup,
    /watch\(\(\) => workspaceStore\.activeKind/,
    '每次进入都强制回启动设置已废弃：首次之后跟随用户选择',
  )

  // 面板内部仍可自由切换：点供应商看编辑器、点页脚回启动设置。
  assert.match(markup, /function onProviderClick[\s\S]*?pane\.value = 'provider'/)
  assert.match(markup, /function createProvider[\s\S]*?pane\.value = 'provider'/)
  assert.match(markup, /function selectStartup\(\) \{\s*pane\.value = 'startup'/)
})

/**
 * wire 协议是 dsh schema 里写死的三值枚举（`api: z.union(supportedProtocols())`，
 * 值落在集合外会被拒绝服务），所以界面上必须是**列全**的下拉框。
 *
 * 这条断言来自一个真实缺陷：原来用的是输入框 + `datalist`，而 datalist 会按输入框
 * 当前值过滤候选——一条已经写着 `openai-completions` 的路由点开只看得到它自己，
 * 看起来就像「dsh 只提供一种协议」。协议集合也在这里钉死：dsh 上游增删协议时，
 * 这条会失败，提醒有人去核对 `@deepseek-ai/dsh-llm-pi-ai` 的 `PROTOCOLS` 表。
 */
test('the wire protocol field is a closed picker over dsh\'s full protocol set', () => {
  const config = readFileSync(resolve(repoRoot, 'src/types/config.ts'), 'utf8')
  const declared = /export const DSH_API_PROTOCOLS = \[([\s\S]*?)\] as const/.exec(config)
  assert.ok(declared, 'missing DSH_API_PROTOCOLS')
  assert.deepEqual(
    declared[1].match(/'([^']+)'/g)?.map((literal) => literal.slice(1, -1)),
    ['openai-completions', 'openai-responses', 'anthropic-messages'],
  )

  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  // 模型 ID 用 datalist 是合理的（模型名是开放的）；协议这一栏不能再用它。
  assert.doesNotMatch(editor, /<datalist id="dsh-api-protocols"/)
  assert.doesNotMatch(editor, /list="dsh-api-protocols"/)

  const picker = /<select v-model="apiChoice"[\s\S]*?<\/select>/.exec(editor)
  assert.ok(picker, 'the wire protocol must be a select')
  assert.match(picker[0], /v-for="protocol in DSH_API_PROTOCOLS"/)
  // 留空是合法的第四种状态：目录里已有的供应商由目录决定协议。
  assert.match(picker[0], /<option value="">/)
  // 手写进 settings.yaml 的其它协议要有兜底项，否则下拉空白、一保存就被改掉。
  assert.match(picker[0], /v-if="unlistedApi"/)
  assert.match(editor, /const apiChoice = computed/)
  assert.match(editor, /const unlistedApi = computed/)
})

/**
 * 有三个字段**不该以输入框的形式出现在界面上**，理由同一个：留空就等于用 ID，用户
 * 在这里没有要做出的决定。
 *
 * - `apiKeyEnv`（凭据引用名）：令牌的**值**有「认证令牌」栏（存在 dsh 的凭据文件里，
 *   Claude 式的显示/隐藏/修改/保存），引用名沿用文件里已有的；没有时按 dsh 的派生
 *   规则（`deriveDshCredentialRef`）生成并自动补写。用户不手写引用名。
 * - `displayName`（供应商显示名称）：dsh 缺失时回落到路由键（`source.displayName ??
 *   provider`），只是一个可选的美化名；编辑器标题固定为「配置编辑」（与 Claude Code
 *   对齐），回落名显示在左侧清单的选中项上。
 * - `name`（模型显示名称）：同一形状，dsh 缺失时回落到模型 ID（`entry.name ??
 *   base?.name ?? entry.id`）。
 *
 * 反过来，三者都必须留在读写路径上——读进来多少写回去多少，否则启动器一次写入就会
 * 抹掉 dsh 补的那一行（`apiKeyEnv` 还会让密钥从此找不到这条路由，症状是运行时的
 * `MISSING_CREDENTIAL`，不是写入报错）。
 */
test('fields with no user decision stay out of the UI but round-trip untouched', () => {
  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  // 只扫实际渲染的标记：注释里当然会提到这些名字（那正是要留下的解释）。
  const template = editor
    .slice(0, editor.indexOf('<script setup'))
    .replace(/<!--[\s\S]*?-->/g, '')

  assert.doesNotMatch(template, /provider\.apiKeyEnv/, '这一栏没有要给用户的决定')
  assert.doesNotMatch(template, /v-model="provider\.displayName"/, '留空就等于供应商 ID')
  assert.doesNotMatch(template, /v-model="model\.name"/, '留空就等于模型 ID')
  assert.doesNotMatch(template, /模型显示名称/)
  // 编辑器标题与 Claude Code 对齐，固定「配置编辑」；displayName 不再渲染进编辑器，
  // 它的「显示名，没有就用 ID」只留在左侧清单（DshConfigPanel 的 item.displayName）。
  assert.match(template, /<div class="card-title">配置编辑<\/div>/)
  assert.doesNotMatch(template, /provider\.displayName \|\| provider\.id/)
  assert.doesNotMatch(template, /keyRefPlaceholder/)
  // 删掉一列会让窄屏的 nth-child 版式错位，序号必须与子元素顺序对齐。
  assert.match(editor, /\.model-row > :nth-child\(5\) \{ grid-column: 3; grid-row: 1; \}/)
  // 但「为什么没有」要留在源码里，且要指得出出处。
  assert.match(editor, /引用名不设输入框/)
  assert.match(editor, /schema\.setPath\(draft, \["apiKeyEnv"\], keyRef\)/)
  assert.match(editor, /source\.displayName \?\? provider/)
  assert.match(editor, /entry\.name \?\? base\?\.name \?\? entry\.id/)

  // 数据面必须完好：类型、草稿默认值、后端接管字段三处都在。
  const types = readFileSync(resolve(repoRoot, 'src/types/config.ts'), 'utf8')
  assert.match(types, /apiKeyEnv: string \| null/)
  assert.match(types, /displayName: string \| null/)
  assert.match(types, /name: string \| null/)
  assert.match(types, /deriveKeyRef/, '派生规则仍要留在字段说明里')

  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')
  assert.match(store, /apiKeyEnv: null/)
  assert.match(store, /displayName: null/)
  assert.match(store, /name: null/)

  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/dsh_settings.rs'), 'utf8')
  assert.match(rust, /"apiKeyEnv",/)
  assert.match(rust, /fn is_credential_ref_name/)
  assert.match(rust, /if !is_credential_ref_name\(reference\)/)
  // 模型条目的 name 仍要能读能写。
  assert.match(rust, /"name" => model\.name = value\.as_text\(\)/)
})

/**
 * 认证令牌（2026-09-18 新增；同日改为随「写入当前修改」一起保存）：
 * dsh 配置页里要有 Claude Code 同款的令牌管理——显示/隐藏/修改/保存。值的落点是
 * dsh 自己的凭据文件（`.credentials.yaml` 的 `refs` 分节），settings.yaml 里只补
 * 引用名 `apiKeyEnv`（沿用已有的，没有才按 `deriveDshCredentialRef` 派生）。
 *
 * 保存路径：普通修改随供应商写入自动跟随保存（`writeSelectedProvider` 里
 * `saveCredential(saved, true)`），不再有常驻的「保存令牌」按钮。独立按钮只留给
 * 两种必须显式动作的情形：清空 = 移除（带确认对话框）与凭据读取失败。
 * 这条把整条链路的两端都钉住：编辑器的字段与条件按钮行、store 的读写命令与
 * 跟随保存、后端的命令与注册。
 */
test('the dsh editor manages the auth token end to end', () => {
  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')
  // Claude Code 同款组件与语义：SecretField 自带显示/隐藏切换。
  assert.match(editor, /import SecretField from '@\/components\/config\/SecretField\.vue'/)
  assert.match(editor, /label="认证令牌"/)
  // 独立按钮行只在「清空 = 移除」或读取失败时出现：移除是破坏性动作，
  // 必须显式确认，不随「写入当前修改」自动执行。
  assert.match(editor, /v-if="showTokenActionRow"/)
  assert.match(editor, /移除令牌/)
  assert.match(editor, /将移除供应商/)
  assert.match(editor, /kind: 'warning'/)

  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')
  assert.match(store, /dsh_read_credential/)
  assert.match(store, /dsh_save_credential/)
  assert.match(store, /deriveDshCredentialRef/)
  // 跟随保存：供应商写入成功后自动保存脏的令牌草稿（quiet 模式，状态条由
  // 写入方组合）；空草稿（移除）与读取失败不自动保存。
  assert.match(store, /await refreshCredentials\(\)/)
  assert.match(store, /saveCredential\(saved, true\)/)
  assert.match(store, /credentialDraftOf\(saved\)\.trim\(\) !== ''/)
  assert.match(store, /!credentialErrorOf\(saved\)/)

  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/dsh_settings.rs'), 'utf8')
  assert.match(rust, /pub fn dsh_read_credential/)
  assert.match(rust, /pub fn dsh_save_credential/)
  assert.match(rust, /fn derive_credential_ref/)
  // 凭据文件必须走私有写入（POSIX 0600），否则 dsh 拒绝加载。
  assert.match(rust, /write_private_text_atomic/)

  const lib = readFileSync(resolve(repoRoot, 'src-tauri/src/lib.rs'), 'utf8')
  assert.match(lib, /dsh_settings::dsh_read_credential/)
  assert.match(lib, /dsh_settings::dsh_save_credential/)
})

/**
 * 回归（任务 202609181731590000）：令牌在「写入当前修改」时丢失、界面显示为空。
 *
 * 链路：writeSelectedProvider 成功后 `load(true)` 静默重读 → `refreshCredentials`
 * 从凭据文件刷值。而凭据文件要等「保存令牌」才动，所以那时读到的是旧值——若
 * 无条件覆盖 `credentialDrafts`，用户已输入但尚未保存的令牌就被静默吞了（凭据
 * 文件里本来没值时，界面直接显示为空）。修法：刷新只覆盖**干净的**草稿
 * （覆盖的是同一个值，无害），脏草稿（改过还没保存）保留。
 *
 * 第二道回归（2026-09-18「重启后看不到令牌」）：脏判定**必须先于** `credentialStored`
 * 的更新。拿刚读到的新值去比，初始为空的草稿会永远被误判为脏，于是重启后
 * 凭据文件里明明有值，输入框却永远填不上。这里钉死「先判定、后更新」的顺序。
 */
test('refreshing credentials never clobbers an unsaved token draft', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')

  const refresh = /async function refreshCredentials\(\) \{([\s\S]*?)\n  \}/.exec(store)
  assert.ok(refresh, 'missing refreshCredentials in the dsh models store')
  // 两个分支（读成功 / 读失败）都必须：先以旧 stored 判定脏，再更新 stored，
  // 最后只对干净草稿写值。判定写进局部变量，顺序由断言的位置关系保证。
  // 「没读过的草稿不判脏」（undefined !== '' 不算脏）——否则 adopt 重建后新对象
  // 永远被误判为脏，文件里的值填不进输入框（2026-09-18「写入配置后令牌变空」）。
  const branches = refresh[1].match(/const dirty = draftValue !== undefined && draftValue !== credentialStored\.get\(draft\)/g)
  assert.equal(branches?.length, 2, 'both branches must snapshot dirtiness before touching credentialStored')
  const guardedWrites = refresh[1].match(/if \(!dirty\) credentialDrafts\.set\(draft,/g)
  assert.equal(guardedWrites?.length, 2, 'both branches must guard the draft write with the pre-update dirtiness')
  // 先判定后更新：dirty 的快照必须出现在 credentialStored 赋值之前。
  for (const match of refresh[1].matchAll(/const dirty = [\s\S]*?(?=credentialStored\.set\(draft,)/g)) {
    assert.ok(match[0].length > 0)
  }
  assert.ok(
    refresh[1].indexOf('const dirty') < refresh[1].indexOf('credentialStored.set(draft, stored)'),
    'the dirtiness snapshot must precede the credentialStored update in the success branch',
  )
})

/**
 * 回归（2026-09-18，任务：保存配置后令牌输入框钉死在空）：WeakMap 不是响应式的。
 *
 * 实测链路：「写入当前修改」成功 → load(true) → adopt 重建草稿（响应式数组变化，
 * computed 重算，此刻 WeakMap 里新对象没值 → 输入框显示空）→ refreshCredentials
 * 随后把值写进裸 WeakMap——这一步不触发任何响应式更新，computed 永不重算，输入框
 * 钉死在空。重启后因为首次求值晚于 refresh 落地而"恢复正常"。（实测日志：refresh
 * 全部成功读到值，store 数据完好，纯粹是界面没跟上。）
 *
 * 修法：credentialVersion ref 作为响应式版本号——读访问器依赖它、每次写入后自增，
 * 界面才能跟着 refresh 落地重算。这条把它钉死。
 */
test('credential state writes bump a reactive version so the UI re-evaluates', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')

  // 版本号存在且被三个读访问器依赖。
  assert.match(store, /credentialVersion = ref\(0\)/)
  for (const accessor of ['credentialDraftOf', 'credentialStoredOf', 'credentialErrorOf']) {
    const body = new RegExp(`function ${accessor}\\(draft: DshProviderDraft\\): string \\{([\\s\\S]*?)\\n  \\}`).exec(store)
    assert.ok(body, `missing ${accessor}`)
    assert.match(body[1], /void credentialVersion\.value/, `${accessor} must depend on the reactive version`)
  }

  // 每个写入口都必须自增版本号：setCredentialDraft、refreshCredentials（尾部）、
  // adopt（继承桥尾部）、saveCredential（stored 落盘后）、fetchModels（availableModels）。
  const bumps = store.match(/credentialVersion\.value\+\+/g)
  assert.ok(bumps && bumps.length >= 5, `expected at least 5 version bumps, got ${bumps?.length ?? 0}`)
})

/**
 * 回归（2026-09-18，任务：改供应商 ID 令牌消失）：改路由键不该碰界面上的令牌，
 * 真正动凭据的时机是「写入当前修改」——且只是把凭据文件里旧引用名的键换成新名，
 * 值原样保留。
 *
 * 根因：凭据状态曾按供应商 id 字符串索引，改名的瞬间新 id 下什么都没有，输入框
 * 立刻变空白；刷新又按新 id 派生的引用名去读，旧名下的值永远找不回来。修法分
 * 两层，这条把它们钉死：
 *
 * 1. 界面态跟草稿对象走（WeakMap），不跟 id 字符串走——改名过程界面完全无感；
 * 2. 写入时若「id 变了 + 引用名是派生的（没写 apiKeyEnv）+ 旧名下有值」，先调
 *    `dsh_rename_credential_ref` 换名再写 settings.yaml。手写 apiKeyEnv 的供应商
 *    引用名与 id 无关，settings 重建块时整行保留，凭据文件完全不用动。
 */
test('renaming a provider id keeps the token on screen and renames the stored ref on write', () => {
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')

  // 1. 凭据三件套与模型清单都按草稿对象索引（WeakMap），不再按 id 字符串。
  assert.match(store, /credentialDrafts = new WeakMap<DshProviderDraft, string>\(\)/)
  assert.match(store, /credentialStored = new WeakMap<DshProviderDraft, string>\(\)/)
  assert.match(store, /credentialErrors = new WeakMap<DshProviderDraft, string>\(\)/)
  assert.match(store, /availableModels = new WeakMap<DshProviderDraft, string\[\]>\(\)/)
  assert.doesNotMatch(store, /credentialDrafts\[draft\.id\]/)

  // 1b. adopt 重建 drafts（对象全换）时必须把旧对象上**未保存的**凭据状态按
  // 路由键继承到新对象——否则「写入当前修改」后 load(true) 一重建，输入框立刻
  // 空白（2026-09-18「修改配置后 api key 变空」的根因）。干净的草稿不继承，
  // 由 refreshCredentials 从凭据文件刷成最新。
  const adopt = /function adopt\(next: DshSettingsDocument\) \{([\s\S]*?)\n  \}/.exec(store)
  assert.ok(adopt, 'missing adopt in the dsh models store')
  assert.match(adopt[1], /carried = new Map/)
  assert.match(adopt[1], /credentialDrafts\.get\(old\)/)
  assert.match(adopt[1], /credentialErrors\.get\(old\)/)
  assert.match(adopt[1], /credentialDrafts\.set\(draft, state\.draft\)/)
  assert.match(adopt[1], /credentialErrors\.set\(draft, state\.error\)/)

  // 2. 写入路径：id 改名且引用名派生时，先换凭据引用名再写 settings.yaml。
  const write = /async function writeSelectedProvider\(\) \{([\s\S]*?)\n  \}/.exec(store)
  assert.ok(write, 'missing writeSelectedProvider in the dsh models store')
  assert.match(write[1], /draft\.originalId !== draft\.id/)
  assert.match(write[1], /!draft\.apiKeyEnv\?\.trim\(\)/)
  assert.match(write[1], /deriveDshCredentialRef\(draft\.originalId\)/)
  assert.match(write[1], /dsh_rename_credential_ref/)
  // 换名必须在写 settings.yaml 之前（settings 侧重建块引用名不变，先换名才不断链）。
  assert.ok(
    write[1].indexOf('dsh_rename_credential_ref') < write[1].indexOf('dsh_write_provider'),
    'the credential ref rename must happen before the settings write',
  )

  // 3. 后端：换名命令按行级手术实现（值原样保留），并注册进命令表。
  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/dsh_settings.rs'), 'utf8')
  assert.match(rust, /pub fn dsh_rename_credential_ref/)
  assert.match(rust, /fn rename_credential_ref_in_text/)
  const lib = readFileSync(resolve(repoRoot, 'src-tauri/src/lib.rs'), 'utf8')
  assert.match(lib, /dsh_settings::dsh_rename_credential_ref/)
})

/**
 * 「获取模型」（2026-09-18，任务 202609181708380000）：API 地址右侧的按钮按当前
 * 地址与令牌去网关拉模型清单，结果填进「添加模型」输入框右侧的下拉——与 Claude
 * Code 配置页同一个形状（ModelField 的输入框 + 下拉）。选中一项只是把 ID 填进
 * 输入框，仍由「添加模型」确认：下拉里一点就加，误触的代价比多点一次按钮高。
 * 清单只进内存，不写进 settings.yaml；后端与 Claude 共用同一条多协议探测链路。
 */
test('the dsh editor fetches gateway models into a picker next to the add-model input', () => {
  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')

  // 按钮在 API 地址那一行、输入框右侧；同一时间只允许一个获取在跑。
  const apiRow = /<label class="field-label">API 地址<\/label>([\s\S]*?)<\/button>/.exec(editor)
  assert.ok(apiRow, 'missing the 获取模型 button on the API 地址 row')
  assert.match(apiRow[1], /v-model="provider\.baseUrl"/)
  assert.match(apiRow[1], /@click="store\.fetchModels\(provider\)"/)
  assert.match(apiRow[1], /获取中…' : '获取模型'/)
  assert.match(apiRow[1], /:disabled="store\.modelsFetchingId !== null"/)

  // 下拉在「添加模型」输入框与按钮之间，候选来自当前供应商的获取结果；
  // 没获取过就禁用并直说「未获取」，选中只是填进输入框（change 进 modelDraft）。
  const addRow = /<div class="model-add-row">([\s\S]*?)<\/div>\s*\n/.exec(editor)
  assert.ok(addRow, 'missing the model add row')
  assert.ok(
    addRow[1].indexOf('v-model="modelDraft"') < addRow[1].indexOf('@change="onFetchedModelPick"')
      && addRow[1].indexOf('@change="onFetchedModelPick"') < addRow[1].indexOf('@click="addModel"'),
    'the picker must sit between the input and the 添加模型 button',
  )
  assert.match(addRow[1], /v-for="id in fetchedModels"/)
  assert.match(addRow[1], /fetchedModels\.length \? '选择模型' : '未获取'/)
  assert.match(addRow[1], /:disabled="fetchedModels\.length === 0"/)
  assert.match(editor, /const fetchedModels = computed/)
  assert.match(editor, /store\.availableModelsOf\(provider\.value\)/)
  assert.match(editor, /function onFetchedModelPick[\s\S]*?modelDraft\.value = value/)

  // store：令牌取「认证令牌」栏的当前值（未保存的修改也算数），结果按供应商存。
  const store = readFileSync(resolve(repoRoot, 'src/stores/dshModels.ts'), 'utf8')
  assert.match(store, /async function fetchModels\(provider: DshProviderDraft\)/)
  assert.match(store, /invoke<string\[\]>\('fetch_dsh_models'/)
  assert.match(store, /authToken: credentialDraftOf\(provider\)\.trim\(\)/)
  assert.match(store, /availableModels\.set\(provider, models\)/)
  assert.match(store, /请先填写 API 地址，再获取模型。/)

  // 后端：与 Claude 同一条多协议探测链路，命令按 CLI 分名并注册。
  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/model_fetcher.rs'), 'utf8')
  assert.match(rust, /pub async fn fetch_dsh_models[\s\S]*?fetch_provider_models\(&base_url, &auth_token\)/)
  const lib = readFileSync(resolve(repoRoot, 'src-tauri/src/lib.rs'), 'utf8')
  assert.match(lib, /model_fetcher::fetch_dsh_models/)
})

/**
 * 思考档位是**按模型**的能力：同一个供应商下的模型对它并不一致，做成供应商级
 * 开关只会「设成某个值、然后被一部分模型拒绝」。这条断言把 dsh 自己的设计结论
 * （见 @deepseek-ai/dsh-client-ui-settings-models 的 ProviderEditor 注释）钉住。
 */
test('reasoning effort is edited per model, with a wire value per level', () => {
  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')

  // 档位编辑出现在模型卡片里，而不是供应商字段里。
  const modelCard = /<div class="model-reasoning">([\s\S]*?)<\/div>\s*<\/div>\s*<\/div>/.exec(editor)
  assert.ok(modelCard, 'missing the per-model reasoning block')
  assert.match(modelCard[1], /v-for="level in DSH_THINKING_LEVELS"/)
  assert.match(modelCard[1], /store\.toggleReasoningLevel\(model, level\)/)

  // 每一档都能单独填「真正发给网关的值」——两者可以不同（max → ultra）。
  // 但这组输入框默认**折叠**（2026-09-18 任务 202609182134580000：勾选档位后
  // 铺开一整组输入框纯属复读，wire 默认就是档位名本身），由「自定义发送值」
  // 折叠入口展开；已有非同名映射（wire ≠ 档位名）的模型默认展开，避免幽灵字段。
  assert.match(editor, /v-for="item in model\.reasoningEfforts"/)
  assert.match(editor, /store\.setReasoningWire\(item,/)
  assert.match(editor, /留空 = 不发送参数/)
  assert.match(editor, /wires-toggle/)
  assert.match(editor, /自定义发送值/)
  assert.match(editor, /v-if="isWireExpanded\(model\)"/)
  assert.match(editor, /hasCustomWire/)

  // 档位集合来自共享常量，不在组件里另抄一份。
  assert.match(editor, /import \{[^}]*DSH_THINKING_LEVELS[^}]*\} from '@\/types\/config'/)
})

/**
 * 配置界面要写明「这套字段照哪一版 dsh 写的」。dsh 仍在迭代，用户和后来改这份
 * 代码的人都得先看到版本号，才知道该去哪一版文档里核对。版本号本身留在界面上
 * （右侧「写入说明」最后一行），而**去哪查**这类排查指引只留在源码注释里——
 * 界面不再摊开文档路径。
 */
test('the dsh panel states the supported dsh version and where to look when it drifts', () => {
  const shell = readFileSync(resolve(repoRoot, SHELL), 'utf8')
  const editor = readFileSync(resolve(repoRoot, EDITOR), 'utf8')

  // 版本号来自后端返回的 supportedVersion（单一真源在 dsh_settings.rs），
  // 组件只负责显示，不自己写死一份。
  assert.match(shell, /store\.supportedVersion \|\| '0\.1\.5-rc\.1'/)
  assert.match(shell, /本页面的字段对应 dsh <strong>v\{\{ supportedVersionLabel \}\}<\/strong>/)

  // 界面上只留「对应哪一版 + 开发者预览版」这一句；文档路径只在注释里，
  // 不占界面。dsh 自己的英文说法（developer preview）留在注释里就够了。
  const shellTemplate = shell.slice(0, shell.indexOf('<script setup'))
  assert.match(shellTemplate, /开发者预览版/)
  assert.doesNotMatch(shellTemplate, /developer preview/)
  assert.doesNotMatch(shellTemplate, /deepseek-ai\/deepseek-harness/)
  assert.doesNotMatch(shellTemplate, /config-catalog\.zh\.md/)

  assert.match(shell, /官方称 developer preview/)
  assert.match(shell, /deepseek-ai\/deepseek-harness/)
  assert.match(shell, /docs\/config-catalog\.zh\.md/)
  assert.match(shell, /docs\/user\/guide\/providers\.zh\.md/)

  // 源码注释里也要有：dsh 仍在迭代、失效时去哪查。界面文案会被改掉，注释才是
  // 后来维护的人真正会读到的地方——所以后端那份注释同样钉住。
  assert.match(editor, /developer preview/)
  assert.match(editor, /dsh_settings\.rs/)
  assert.match(editor, /docs\/config-catalog\.zh\.md/)

  const rust = readFileSync(resolve(repoRoot, 'src-tauri/src/dsh_settings.rs'), 'utf8')
  assert.match(rust, /DSH_SETTINGS_SUPPORTED_VERSION: &str = "0\.1\.5-rc\.1"/)
  assert.match(rust, /developer preview/)
  assert.match(rust, /github\.com\/deepseek-ai\/deepseek-harness/)
  assert.match(rust, /docs\/config-catalog\.zh\.md/)
})

/**
 * The reported bug: with Tailscale installed, 「复制链接」 handed out the 100.x
 * address dsh reported as its LAN one, which no phone on the same Wi-Fi can
 * open. The panel now lists 本机 / 局域网 / Tailscale from the runtime address
 * list, and every action lives on the row it belongs to.
 */
test('every address row carries 二维码 / 复制 / 打开网页, in that order', () => {
  const markup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')
  const start = markup.indexOf('<div class="address-list">')
  const end = markup.indexOf('<div class="runtime-entry">')
  assert.ok(start !== -1 && end > start, 'missing dsh access list')
  const block = markup.slice(start, end)

  // Rows come from the runtime address list, so a new interface or a tailnet
  // appears without another UI change.
  assert.match(block, /v-for="row in rows"/)
  assert.match(block, /\{\{ row\.label \}\}/)
  assert.match(block, /\{\{ row\.display \}\}/)

  // 二维码 sits left of 复制, and 打开网页 right of it, all inside the row.
  assert.match(
    block,
    /<li[^>]*v-for="row in rows"[\s\S]*?@click="openQr\(row\)"[\s\S]*?二维码[\s\S]*?@click="copyRowLink\(row\)"[\s\S]*?复制[\s\S]*?@click="openRowLink\(row\)"[\s\S]*?打开网页[\s\S]*?<\/li>/,
  )
  // QR only where another device can actually land: loopback is excluded, so a
  // 127.0.0.1 row has no QR button.
  assert.match(block, /v-if="needsQr\(row\)"[\s\S]*?@click="openQr\(row\)"/)
  assert.match(markup, /function needsQr\(row: DshAddressRow\): boolean \{[\s\S]*?row\.kind === 'lan' \|\| row\.kind === 'tailscale'/)

  assert.match(markup, /function copyRowLink\(row: DshAddressRow\) \{[\s\S]*?copyAddress\(row\)/)
  assert.match(
    markup,
    /async function openRowLink\(row: DshAddressRow\) \{[\s\S]*?await resolveAddress\(row\)[\s\S]*?await open\(value\)/,
  )
  assert.match(markup, /import \{ open \} from '@tauri-apps\/plugin-shell'/)
})

/**
 * The dsh panel's 启动前检测 entry (a secondary button plus a hint sentence) is
 * replaced by a single「进入DeepSeek Harness」jump to the dsh tab. The button
 * must read as a destination, not an action or a warning: a violet of its own,
 * explicitly not the primary blue, the success green, or a warning/danger hue.
 */
test('the dsh panel replaces 启动前检测 with a distinctively coloured jump button', () => {
  const markup = readFileSync(resolve(repoRoot, STARTUP), 'utf8')

  // The old entry is gone: no preflight button, no hint sentence.
  assert.doesNotMatch(markup, /preflight-entry|openPreflight|启动前检测/)

  // The new entry emits the jump; the chain CliDshPanel → ConfigWorkspace →
  // App.vue carries it to openCliTab('dsh'), the same path as the title bar.
  assert.match(markup, /class="btn runtime-entry__button"[^>]*@click="emit\('open-runtime'\)"/)
  assert.match(markup, /进入DeepSeek Harness/)

  const cliPanel = readFileSync(resolve(repoRoot, 'src/components/cli/CliDshPanel.vue'), 'utf8')
  assert.match(cliPanel, /@open-runtime="emit\('open-runtime'\)"/)
  const workspace = readFileSync(resolve(repoRoot, 'src/components/config/ConfigWorkspace.vue'), 'utf8')
  assert.match(workspace, /@open-runtime="emit\('open-runtime'\)"/)
  const app = readFileSync(resolve(repoRoot, 'src/App.vue'), 'utf8')
  assert.match(app, /@open-runtime="openDshRuntimeTab"/)
  assert.match(app, /function openDshRuntimeTab\(\) \{[\s\S]*?openCliTab\('dsh'\)/)

  const css = scopedStyle(STARTUP)
  const button = declarations(css, '.runtime-entry__button')
  const background = button.get('background-color') ?? ''
  assert.ok(background, 'the jump button must carry its own colour')
  // 紫色 #6B5CE7 的 RGB：蓝最高、红次之、绿最低。主色蓝的绿分量远高于红，警告/
  // 危险色的红远高于蓝——这条规则把「特殊但不是警告色」落成可检查的约束。
  const rgb = /#([0-9A-Fa-f]{2})([0-9A-Fa-f]{2})([0-9A-Fa-f]{2})/.exec(background)
  assert.ok(rgb, `background ${background} must be a hex colour the test can reason about`)
  const [r, g, b] = rgb.slice(1).map((hex) => Number.parseInt(hex, 16))
  assert.ok(b > r && r > g, `background ${background} must be a violet, not blue/amber/red`)
  assert.equal(button.get('color'), '#FFFFFF')

  // 跳转按钮独占一行，靠右；设置入口已经搬到侧边栏页脚，这里不再有它。
  assert.equal(declarations(css, '.runtime-entry').get('justify-content'), 'flex-end')
})
