//! dsh 用户设置里「模型与供应商」部分的读写，以及 dsh 凭据文件
//! （`$DSH_HOME/.credentials.yaml`）里认证令牌值的读写。
//!
//! # 两种布局，按 dsh 版本分派
//!
//! dsh 在 **0.2.0** 把用户设置文档换掉了（[`DshSettingsLayout`]）：
//!
//! - **0.1.x（`LegacyYaml`，对齐 v0.1.5-rc.1）**：`$DSH_HOME/settings.yaml`，
//!   顶层是映射，供应商在 `llm-pi-ai.providers` 下；dsh 热重载这份文件。
//! - **0.2.0 起（`ProfilePatch`，对齐 v0.2.0-rc.2）**：dsh 启动时把
//!   `settings.yaml` 一次性改名为 `settings.yaml.imported` 并导入 profile
//!   （`@deepseek-ai/dsh-settings` 的 `importLegacyDocument`），之后的真源是
//!   `$DSH_HOME/profiles/web/cordis.patch.yml`——顶层是 loader patch **序列**，
//!   供应商在 `- id: llm-pi-ai` 那条 entry 的 `config.providers` 下，与其它
//!   entry（`ui-theme`、`permission` …）混排。0.2.0 **不再热重载**这份文件
//!   （loader 不 watch 它），写入要重启 dsh 才生效。
//!
//! 另一个 0.2.0 的硬约束：自定义路由**必须声明 `api`**（`openai-completions` /
//! `openai-responses` / `anthropic-messages`）。缺了它 dsh 装配 llm-pi-ai 时抛
//! `PiAiCatalogError`，整条配置被拒——包括一次性导入在内——且只在日志里留一句
//! warn（2026-10-05「strata 导入后消失」的根因）。所以 patch 布局下写盘前就把
//! 这条挡住（[`enforce_patch_route_rules`]）。
//!
//! # 如果哪天读写失效了
//!
//! dsh 官方自述仍是 developer preview、会有 breaking changes，设置的层级、
//! 字段名与语义都可能随版本变动——所以这里的解析**只认它需要的字段**，遇到不
//! 认识的语法一律报错而不是猜（fail loud）。按这个顺序去查当前版本的实现：
//!
//! 1. 官方仓库 <https://github.com/deepseek-ai/deepseek-harness> 的
//!    `docs/user/guide/providers.zh.md`（用户指南）与 `docs/config-catalog.zh.md`
//!    （生成式配置目录，逐字段真源）。这两份**不随 npm 包发布**，只能看仓库。
//! 2. 包内第一手文档：`npx --yes @deepseek-ai/dsh -V` 拿到版本后，到
//!    `%LOCALAPPDATA%\npm-cache\_npx\*\node_modules\@deepseek-ai\` 下读
//!    `dsh-llm-pi-ai/README.zh.md`（供应方路由与模型条目的字段表就在这里），
//!    以及 `dsh-llm-pi-ai/lib/types/catalog.d.ts`（`reasoningEfforts` 等类型）；
//!    patch 布局本身看 `dsh-settings/lib/index.js` 与 `dsh-config-editor/lib/index.js`。
//! 3. 运行时真源：`npx --yes @deepseek-ai/dsh web --dump-config`。
//!
//! # 为什么不用 YAML 库
//!
//! 两份文档都是用户手改、可版本化的文件，注释和字段顺序都有价值。为一个文件
//! 引入 YAML 解析依赖并不划算，所以这里的做法是**按行做最小手术**：只重写启动
//! 器负责的那几个字段，块内其它字段、其它供应商、其它 entry（patch 布局里可能
//! 带 `!!js` 表达式，定位边界即可、**不解析内容**）都**逐字节保留**。
//!
//! 代价是：被启动器改写过的那个字段（连同它更深的子行）会按规范形式重排，
//! 它**内部**的注释不保留。没有改动过的供应商整块原样保留，注释不受影响。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_transaction::{write_private_text_atomic, write_text_atomic};

/// 旧布局（0.1.x）对齐的 dsh 版本。前端把它显示在配置界面上，让用户知道这套
/// 字段是照哪一版写的；升级 dsh 后如果设置页出现异常，第一件事就是核对这个
/// 版本号。
pub const DSH_SETTINGS_SUPPORTED_VERSION_LEGACY: &str = "0.1.5-rc.1";
/// 新布局（0.2.0 起的 profile patch）对齐的 dsh 版本。
pub const DSH_SETTINGS_SUPPORTED_VERSION_PATCH: &str = "0.2.0-rc.2";

/// dsh 用户数据目录名（与 `@deepseek-ai/dsh-home-paths` 的 `DSH_HOME_DIR_NAME` 一致）。
const DSH_HOME_DIR_NAME: &str = ".dsh";
/// 覆盖 dsh 用户数据目录的环境变量（与 `dsh-home-paths` 的 `DSH_HOME_ENV` 一致）。
const DSH_HOME_ENV: &str = "DSH_HOME";
/// 旧布局的设置文档文件名（0.1.x；0.2.0 起 dsh 启动时会把它改名导入）。
const DSH_SETTINGS_FILE_NAME: &str = "settings.yaml";
/// dsh 0.2.0 一次性导入后把 `settings.yaml` 改名成的残留文件。
const DSH_IMPORTED_FILE_NAME: &str = "settings.yaml.imported";
/// 新布局的 profile patch 文件名（0.2.0 起）。
const DSH_PROFILE_PATCH_FILE_NAME: &str = "cordis.patch.yml";
/// `dsh web` 固定 boot 的 profile 名（等价于 `dsh --profile web`）。
const DSH_WEB_PROFILE_NAME: &str = "web";

/// pi-ai 适配器的设置分区名（旧布局的顶层键、新布局的 entry id）。
const PI_AI_SECTION: &str = "llm-pi-ai";
/// patch entry 里携带配置的字段名。
const CONFIG_KEY: &str = "config";
/// llm-pi-ai entry 的包名（新增 entry 时按 dsh 自己写出的形状带上）。
const PI_AI_ENTRY_NAME: &str = "@deepseek-ai/dsh-llm-pi-ai";
/// 分区里的供应方字典名。
const PROVIDERS_KEY: &str = "providers";

/// dsh/pi-ai 允许的思考档位，升序排列（与 `dsh-llm-pi-ai` 的 `THINKING_LEVELS`
/// 完全一致）。声明了别的键会在 dsh 侧被拒，所以这里先挡住。
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 启动器负责编辑的供应商字段。其余字段（`compat` / `headers` / `retryPolicy` /
/// `thinkingBudgets` / `modelOverrides` …）一律不动，只把它们**存在**这件事告诉前端。
const MANAGED_PROVIDER_KEYS: [&str; 6] = [
    "apiKeyEnv",
    "displayName",
    "api",
    "baseURL",
    "reasoning",
    "models",
];

/// 启动器负责编辑的模型字段。
const MANAGED_MODEL_KEYS: [&str; 6] = [
    "id",
    "name",
    "contextWindow",
    "maxTokens",
    "input",
    "reasoningEfforts",
];

// ---------------------------------------------------------------------------
// 对外数据结构
// ---------------------------------------------------------------------------

/// 设置文档的布局。dsh 在 0.2.0 换了配置文件（见模块头），两种布局的读写
/// 都保留，按检测到的 dsh 版本分派（[`settings_layout`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DshSettingsLayout {
    /// dsh 0.1.x：`$DSH_HOME/settings.yaml`，顶层 `llm-pi-ai:` 映射。
    #[serde(rename = "legacy-yaml")]
    LegacyYaml,
    /// dsh 0.2.0 起：`$DSH_HOME/profiles/web/cordis.patch.yml`，顶层是 loader
    /// patch 序列，供应商在 `- id: llm-pi-ai` 那条 entry 的 `config.providers` 下。
    #[serde(rename = "profile-patch")]
    ProfilePatch,
}

impl DshSettingsLayout {
    /// 该布局对齐的 dsh 版本（界面展示用）。
    fn supported_version(self) -> &'static str {
        match self {
            DshSettingsLayout::LegacyYaml => DSH_SETTINGS_SUPPORTED_VERSION_LEGACY,
            DshSettingsLayout::ProfilePatch => DSH_SETTINGS_SUPPORTED_VERSION_PATCH,
        }
    }
}

/// 设置文档的读取结果。`providers` 是已经解析好的结构化数据；前端不再自己解析
/// YAML，因此这里也是「文件里现在到底有什么」的唯一来源。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshSettingsDocument {
    /// 设置文件的绝对路径（哪种布局指哪个文件）。
    pub path: String,
    /// 文件是否已存在。不存在时 `providers` 为空，仍然可以正常写入（会创建文件）。
    pub exists: bool,
    /// 文件内容的 sha256。写回时必须原样带回，用于检测「读之后被别人改过」。
    pub revision: String,
    /// 当前布局对齐的 dsh 版本。
    pub supported_version: String,
    /// 本次读写按哪种布局进行（由检测到的 dsh 版本决定）。
    pub layout: DshSettingsLayout,
    /// 解析出的 pi-ai 供应方，顺序与文件中的顺序一致。
    pub providers: Vec<DshProviderProfile>,
    /// `settings.yaml.imported` 里还没回到当前文档的供应商（dsh 0.2.0 一次性
    /// 导入时丢弃的那些）。没有可恢复的（或文件不存在）时为 None。
    pub legacy_import: Option<DshLegacyImport>,
}

/// `$DSH_HOME/settings.yaml.imported` 的可恢复内容。
///
/// dsh 0.2.0 的一次性导入是「先改名再逐节导入，某节被拒就只留在改名后的文件里」
/// （`dsh-settings` 的 `importLegacyDocument`）——被拒的供应商不会丢数据，但也
/// 不会自己回来。启动器据此提供一键恢复（见 [`dsh_restore_imported_providers`]）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshLegacyImport {
    /// 导入残留文件的绝对路径。
    pub path: String,
    /// 文件在但解析失败时的原因（此时 `candidates` 为空，恢复按钮不可用）。
    pub parse_error: Option<String>,
    /// 还没回到当前文档的供应商，顺序与残留文件中的顺序一致。
    pub candidates: Vec<DshImportedProvider>,
}

/// 一个可恢复的供应商：完整档案 + 恢复前必须先解决的问题。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshImportedProvider {
    #[serde(flatten)]
    pub provider: DshProviderProfile,
    /// 空 = 可直接恢复；非空 = 每条是一句人话（多半是被 dsh 0.2.0 拒掉的原因，
    /// 例如自定义路由缺 `api`），用户要先手工补上才能恢复。
    pub problems: Vec<String>,
}

/// 一个 pi-ai 供应方路由（`llm-pi-ai.providers.<id>`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshProviderProfile {
    /// 路由键。请求用它选路由，凭据引用也按它派生，改动等于换一个供应商。
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// wire 协议。pi-ai 目录里有的路由可以留空（由目录决定），手工声明的路由必须有。
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// 凭据引用名（环境变量名）。**这里存的是引用，不是密钥**：dsh 把真正的密钥
    /// 放在 `$DSH_HOME/.credentials.yaml`。引用名在配置页里没有输入框——保存令牌时
    /// 沿用文件里已有的名字，没有才按 dsh 的派生规则生成并自动补写（见
    /// [`dsh_save_credential`]）。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 路由级默认思考档（请求没指名档位时用它）。
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub models: Vec<DshModelProfile>,
    /// 该供应商下启动器**不编辑**的字段名，仅用于界面提示「还有字段由
    /// settings.yaml 管理」。这些字段在写回时逐字节保留。
    #[serde(default)]
    pub unmanaged_fields: Vec<String>,
    /// 是否是「自定义」路由：pi-ai 内置目录在该路由键下不提供任何内容，协议、端点、
    /// 模型全由 settings.yaml 声明。等价于 dsh 自己设置页里那枚「自定义」标签
    /// （上游 `entry.declared === true`，`dsh-client-ui-settings-models`）。
    ///
    /// **不是文件里的字段**：读取时按已安装的目录清单算出来（见
    /// [`mark_custom_routes`]），写回时被忽略——渲染只认 [`MANAGED_PROVIDER_KEYS`]。
    /// 配置界面据此只在左侧列出自定义路由。
    #[serde(default)]
    pub custom: bool,
}

/// 一个模型条目（`models[]` 或 `modelOverrides` 里的一项）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshModelProfile {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// 请求模态，例如 `["text", "image"]`。手工声明的模型默认按纯文本对待，
    /// 要收图片必须显式声明。
    #[serde(default)]
    pub input: Option<Vec<String>>,
    /// `true` 表示显式写 `reasoningEfforts: false`（声明为非推理模型）。
    #[serde(default)]
    pub reasoning_disabled: bool,
    /// 声明的思考档位。**键是 dsh 的档位 ID，值是真正发给网关的拼写。**
    #[serde(default)]
    pub reasoning_efforts: Vec<DshReasoningLevel>,
    /// 该模型下启动器**不编辑**的字段名（例如模型级的 `compat`），仅用于界面提示。
    #[serde(default)]
    pub unmanaged_fields: Vec<String>,
}

/// 一个思考档位的声明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshReasoningLevel {
    /// dsh 档位 ID，取值见 [`THINKING_LEVELS`]。
    pub level: String,
    /// 发给提供方的 wire 值。只有 `off` 允许为空（表示「支持但不发参数」）。
    #[serde(default)]
    pub wire: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshWriteProviderRequest {
    /// 读取时拿到的 revision；与当前文件不一致就拒绝写入。
    pub base_revision: String,
    /// 文件里原来的路由键。`None` 表示新增；`Some` 且与 `provider.id` 不同表示改名。
    #[serde(default)]
    pub original_id: Option<String>,
    pub provider: DshProviderProfile,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshDeleteProviderRequest {
    pub base_revision: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshSettingsWriteResult {
    pub path: String,
    pub revision: String,
    /// 备份文件路径（写入成功时必定存在）。
    pub backup_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshRestoreImportedRequest {
    /// 读取时拿到的 revision；与当前文件不一致就拒绝写入。
    pub base_revision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshRestoreImportedResult {
    pub path: String,
    pub revision: String,
    pub backup_path: String,
    /// 已恢复的供应商路由键，按残留文件中的顺序。
    pub restored: Vec<String>,
    /// 没能恢复的供应商：每条 problems 就是当初被 dsh 拒掉/校验不过的原因，
    /// 需要手工补齐字段后再恢复。
    pub skipped: Vec<DshImportedProvider>,
}

// ---------------------------------------------------------------------------
// 路径
// ---------------------------------------------------------------------------

/// 展开开头的 `~`。dsh 自己的 `expandHomePath` 也做这件事，而 `DSH_HOME` 允许
/// 写成 `~/.dsh-other` 这种形式。
fn expand_home(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    let rest = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"));
    match rest {
        None => Ok(PathBuf::from(trimmed)),
        Some(suffix) => {
            let home = dirs::home_dir()
                .ok_or_else(|| "无法定位用户主目录，因而无法展开 DSH_HOME 里的 ~".to_string())?;
            Ok(home.join(suffix))
        }
    }
}

/// dsh 用户数据目录：`$DSH_HOME` 优先，否则 `~/.dsh`。
///
/// 与 `@deepseek-ai/dsh-home-paths` 的 `resolveDshHome` 保持一致——设置文件落在
/// 哪里完全由它决定，写错位置会变成「改了但 dsh 不认」。
pub fn dsh_home() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os(DSH_HOME_ENV) {
        let raw = value.to_string_lossy();
        if !raw.trim().is_empty() {
            return expand_home(&raw);
        }
    }
    let home = dirs::home_dir()
        .ok_or_else(|| "无法定位用户主目录，因而无法找到 dsh 设置文件".to_string())?;
    Ok(home.join(DSH_HOME_DIR_NAME))
}

/// 旧布局设置文件（`settings.yaml`）的完整路径。
pub fn dsh_settings_path() -> Result<PathBuf, String> {
    Ok(dsh_home()?.join(DSH_SETTINGS_FILE_NAME))
}

/// 一次性导入残留文件（`settings.yaml.imported`）的完整路径。
fn dsh_imported_settings_path() -> Result<PathBuf, String> {
    Ok(dsh_home()?.join(DSH_IMPORTED_FILE_NAME))
}

/// 新布局设置文件（`profiles/web/cordis.patch.yml`）的完整路径。
/// `dsh web` 固定 boot `web` profile，所以路径不含变量。
pub fn dsh_profile_patch_path() -> Result<PathBuf, String> {
    Ok(dsh_home()?
        .join(DSH_PROFILES_DIR)
        .join(DSH_WEB_PROFILE_NAME)
        .join(DSH_PROFILE_PATCH_FILE_NAME))
}

/// 布局对应的设置文件路径。
fn document_path(layout: DshSettingsLayout) -> Result<PathBuf, String> {
    match layout {
        DshSettingsLayout::LegacyYaml => dsh_settings_path(),
        DshSettingsLayout::ProfilePatch => dsh_profile_patch_path(),
    }
}

/// 从版本号判定布局：0.2.0 起是 profile patch。解析不了（不是数字开头的
/// major.minor）返回 None，调用方走嗅探兜底。
fn layout_for_version(version: &str) -> Option<DshSettingsLayout> {
    let mut parts = version.trim().split(['.', '-']);
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    Some(if major > 0 || minor >= 2 {
        DshSettingsLayout::ProfilePatch
    } else {
        DshSettingsLayout::LegacyYaml
    })
}

/// 当前应按哪种布局读写。
///
/// 优先跟着「启动器会启动的 dsh 版本」走（pin 的版本就是启动用的版本）；没 pin
/// 过时按文件系统嗅探：patch 文件在 → 新布局；只有 settings.yaml → 旧布局；都
/// 没有 → 新布局（现在 npx 装的都是 0.2.0 起，新机器不该再写一份 dsh 启动时还要
/// 一次性导入的旧文件）。
fn settings_layout() -> DshSettingsLayout {
    if let Some(version) = crate::persistent_state::load_dsh_pinned_version() {
        if let Some(layout) = layout_for_version(&version) {
            return layout;
        }
    }
    if let Ok(path) = dsh_profile_patch_path() {
        if path.is_file() {
            return DshSettingsLayout::ProfilePatch;
        }
    }
    match dsh_settings_path() {
        Ok(path) if path.is_file() => DshSettingsLayout::LegacyYaml,
        _ => DshSettingsLayout::ProfilePatch,
    }
}

/// dsh profile 的依赖目录名（0.1.x 里 pi-ai 目录装在 `profiles/node_modules`）。
const DSH_PROFILES_DIR: &str = "profiles";
const DSH_PROFILE_MODULES_DIR: &str = "node_modules";
/// 提供内置供应商目录的包（`dsh-llm-pi-ai` 从它取 `getBuiltinProviders()`）。
const PI_AI_SCOPE_DIR: &str = "@earendil-works";
const PI_AI_PACKAGE_DIR: &str = "pi-ai";

/// pi-ai 目录清单（`dist/providers/data/.manifest.json`）的相对路径；`files`
/// 键就是每个目录路由（`<路由键>.json`），与上游 `getBuiltinProviders()`
/// （即生成表 `MODELS` 的键）逐项一致。
fn pi_ai_manifest_relative() -> PathBuf {
    PathBuf::from(PI_AI_SCOPE_DIR)
        .join(PI_AI_PACKAGE_DIR)
        .join("dist")
        .join("providers")
        .join("data")
        .join(".manifest.json")
}

/// 0.2.0 起 pi-ai 目录随 dsh 包一起装在 npx 缓存条目里（`profiles/node_modules`
/// 不再安装，旧机器上那里可能只剩指向失效缓存的符号链接）。按 pin 的版本找对应
/// 缓存条目；没 pin 或 pin 的条目里读不到清单时，从新到旧找第一个带清单的条目。
fn patch_layout_catalog_manifest() -> Option<PathBuf> {
    let cache = crate::dsh_runtime::npm_cache_dir()?;
    let entries = crate::dsh_runtime::cached_dsh_entries(&cache).ok()?;
    let pinned = crate::persistent_state::load_dsh_pinned_version();
    let pinned = pinned.as_deref();
    // BTreeMap 按版本字符串排序；.rev() 从最新开始。pin 的版本最先试。
    let mut versions: Vec<&str> = Vec::new();
    if let Some(pinned) = pinned {
        versions.push(pinned);
    }
    versions.extend(entries.keys().map(String::as_str).rev());
    let mut tried = std::collections::HashSet::new();
    for version in versions {
        if !tried.insert(version) {
            continue;
        }
        let Some(dirs) = entries.get(version) else {
            continue;
        };
        for dir in dirs {
            let manifest = dir.join(DSH_PROFILE_MODULES_DIR).join(pi_ai_manifest_relative());
            // is_file 跟随符号链接：失效链接（旧缓存已删）自然落空。
            if manifest.is_file() {
                return Some(manifest);
            }
        }
    }
    None
}

/// 布局对应的 pi-ai 目录清单路径；`None` 表示读不到（profile 还没装过、缓存
/// 条目缺失，或上游换了布局）。
fn pi_ai_catalog_manifest_path(layout: DshSettingsLayout) -> Option<PathBuf> {
    match layout {
        DshSettingsLayout::LegacyYaml => Some(
            dsh_home()
                .ok()?
                .join(DSH_PROFILES_DIR)
                .join(DSH_PROFILE_MODULES_DIR)
                .join(pi_ai_manifest_relative()),
        ),
        DshSettingsLayout::ProfilePatch => patch_layout_catalog_manifest(),
    }
}

/// dsh 自带的目录路由集合；`None` 表示读不到（见 [`pi_ai_catalog_manifest_path`]）。
fn installed_catalog_routes(layout: DshSettingsLayout) -> Option<std::collections::HashSet<String>> {
    let raw = std::fs::read_to_string(pi_ai_catalog_manifest_path(layout)?).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let files = manifest.get("files")?.as_object()?;
    let routes: std::collections::HashSet<String> = files
        .keys()
        .filter_map(|name| name.strip_suffix(".json"))
        .map(str::to_string)
        .collect();
    (!routes.is_empty()).then_some(routes)
}

/// 标出哪些路由是自定义的（见 [`DshProviderProfile::custom`]）。
///
/// 判据是「路由键是否在 dsh 自带的目录里」，与 dsh 自己的设置页用同一份数据，
/// 所以两处对同一条路由的判断必然一致。目录清单读不到时**全部按自定义处理**：
/// 界面退化成「全都列出来」，这比把用户自己声明的路由藏起来安全——藏起来的东西
/// 在这个界面里既看不见也改不了。
fn mark_custom_routes(providers: &mut [DshProviderProfile], layout: DshSettingsLayout) {
    let catalog = installed_catalog_routes(layout);
    for provider in providers {
        provider.custom = is_custom_route(&provider.id, catalog.as_ref());
    }
}

/// 一条路由是不是自定义的。目录读不到时按自定义处理（见 [`mark_custom_routes`]）。
fn is_custom_route(id: &str, catalog: Option<&std::collections::HashSet<String>>) -> bool {
    match catalog {
        Some(routes) => !routes.contains(id),
        None => true,
    }
}

fn revision_of(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// 行级工具
// ---------------------------------------------------------------------------

/// 行首空格数。制表符直接拒绝：YAML 不允许用 tab 缩进，而「看起来对齐了但缩进
/// 层级不同」会让按行定位静默错位，那是这里最危险的失败模式。
fn indentation(line: &str) -> Result<usize, String> {
    let mut count = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => count += 1,
            '\t' => {
                return Err(
                    "dsh 设置文件的缩进里出现了制表符，无法安全地按行定位。\
                     请把缩进改成空格后重试（dsh 自己写出的文件只用空格）。"
                        .to_string(),
                )
            }
            _ => break,
        }
    }
    Ok(count)
}

/// 去掉行尾注释。引号内的 `#` 不是注释——`baseURL: 'https://x/#/a'` 这种值里
/// 就带 `#`，切错会把 URL 截断。
fn strip_comment(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if in_single {
            if ch == '\'' {
                // YAML 里 '' 是一个转义的单引号，不结束字符串。
                if chars.get(index + 1) == Some(&'\'') {
                    index += 2;
                    continue;
                }
                in_single = false;
            }
        } else if in_double {
            if ch == '\\' {
                index += 2;
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
        } else {
            match ch {
                '\'' => in_single = true,
                '"' => in_double = true,
                '#' => {
                    // 只有前面是空白或行首的 # 才起始注释。
                    let preceded_by_space = index == 0
                        || matches!(chars[index - 1], ' ' | '\t');
                    if preceded_by_space {
                        return chars[..index].iter().collect();
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    line.to_string()
}

/// 该行去掉注释与首尾空白后的内容；纯注释行或空行返回空串。
fn content_of(line: &str) -> String {
    strip_comment(line).trim().to_string()
}

fn is_blank_or_comment(line: &str) -> bool {
    content_of(line).is_empty()
}

/// 按第一个不在引号内的 `:` 拆成 `(键, 值)`。值可能是空串（块级子节点）。
fn split_key_value(text: &str) -> Option<(String, String)> {
    let mut in_single = false;
    let mut in_double = false;
    for (index, ch) in text.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ':' if !in_single && !in_double => {
                let key = text[..index].trim().to_string();
                let value = text[index + 1..].trim().to_string();
                if key.is_empty() {
                    return None;
                }
                return Some((key, value));
            }
            _ => {}
        }
    }
    None
}

/// 解析一个标量。只接受行内标量：块标量（`|` / `>`）、锚点、别名、标签都拒绝——
/// 这些形式下「值在哪几行」不再是显然的，猜错就会改坏文件。
fn parse_scalar(raw: &str) -> Result<Scalar, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Ok(Scalar::Empty);
    }
    let head = text.chars().next().unwrap_or(' ');
    if matches!(head, '|' | '>') {
        return Err(format!(
            "设置文件里的值 `{text}` 使用了块标量（`|` 或 `>`），启动器只支持行内标量，\
             请改用单行写法。"
        ));
    }
    if matches!(head, '&' | '*') {
        return Err(format!(
            "设置文件里的值 `{text}` 使用了锚点或别名，启动器无法安全定位，请展开后重试。"
        ));
    }
    if head == '!' {
        return Err(format!(
            "设置文件里的值 `{text}` 带 YAML 标签，启动器不解析标签，请去掉后重试。"
        ));
    }
    if text.starts_with('{') || text.starts_with('[') {
        return Ok(Scalar::Flow(text.to_string()));
    }
    if (text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2)
        || (text.starts_with('"') && text.ends_with('"') && text.len() >= 2)
    {
        let inner = &text[1..text.len() - 1];
        let unquoted = if text.starts_with('\'') {
            inner.replace("''", "'")
        } else {
            // 双引号字符串里的转义按 YAML 的常见子集处理；认不出来就原样保留，
            // 不假装自己做了完整的反转义。
            inner.replace("\\\"", "\"").replace("\\\\", "\\")
        };
        return Ok(Scalar::Text(unquoted));
    }
    Ok(Scalar::Text(text.to_string()))
}

#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    /// `key:` 后面什么都没有——值在更深缩进的子节点里。
    Empty,
    Text(String),
    /// 流式集合（`[...]` / `{...}`）。只在少数位置被接受。
    Flow(String),
}

impl Scalar {
    fn as_text(&self) -> Option<String> {
        match self {
            Scalar::Empty => None,
            Scalar::Text(value) => Some(value.clone()),
            Scalar::Flow(value) => Some(value.clone()),
        }
    }
}

/// 一个块里的字段项：`key:` 那一行以及它更深缩进的后续行。
#[derive(Debug, Clone)]
struct FieldItem {
    key: String,
    value: Scalar,
    /// 起始行下标（含）。
    start: usize,
    /// 结束行下标（不含）：下一个同缩进字段项的开始处，或块的结束处。
    end: usize,
}

impl FieldItem {
    /// 实际内容结束的位置：把尾部连续的空行/纯注释行排除在外，这样替换字段时
    /// 不会顺手删掉紧跟其后的注释。
    fn content_end(&self, lines: &[String]) -> usize {
        let mut end = self.end;
        while end > self.start + 1 && is_blank_or_comment(&lines[end - 1]) {
            end -= 1;
        }
        end
    }
}

/// 把 `[start, end)` 内、缩进恰好等于 `indent` 的 `key:` 行切成字段项。
/// 更深的行归属上一个字段项，注释行与空行不产生字段项（因而原样保留）。
fn field_items(
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> Result<Vec<FieldItem>, String> {
    let mut items: Vec<FieldItem> = Vec::new();
    for index in start..end {
        let raw = &lines[index];
        if is_blank_or_comment(raw) {
            continue;
        }
        if indentation(raw)? != indent {
            continue;
        }
        let content = content_of(raw);
        let Some((key, value)) = split_key_value(&content) else {
            continue;
        };
        let scalar = parse_scalar(&value)?;
        items.push(FieldItem {
            key,
            value: scalar,
            start: index,
            end: index + 1,
        });
    }
    for index in 0..items.len() {
        let next = items.get(index + 1).map(|item| item.start).unwrap_or(end);
        items[index].end = next;
    }
    Ok(items)
}

/// 一个块的边界：`key:` 行的下标，以及块内内容的 `[content_start, end)`。
#[derive(Debug, Clone)]
struct Block {
    /// 子内容的起始行（等于 key_line + 1）。
    content_start: usize,
    /// 块结束行（不含）。
    end: usize,
    /// 子内容的缩进；块为空时是 `indent + 2`。
    child_indent: usize,
}

/// 在 `[start, end)` 里找缩进为 `parent_child_indent` 的 `key:`，返回它的块边界。
fn find_block(
    lines: &[String],
    start: usize,
    end: usize,
    child_indent: usize,
    key: &str,
) -> Result<Option<Block>, String> {
    for index in start..end {
        let raw = &lines[index];
        if is_blank_or_comment(raw) {
            continue;
        }
        let line_indent = indentation(raw)?;
        if line_indent != child_indent {
            continue;
        }
        let content = content_of(raw);
        let Some((found_key, value)) = split_key_value(&content) else {
            continue;
        };
        if found_key != key {
            continue;
        }
        if !value.trim().is_empty() {
            return Err(format!(
                "设置文件里的 `{key}:` 写成了行内值 `{value}`，启动器需要它是块级子节点。\
                 请把它展开成多行后重试。"
            ));
        }
        // 块在下一个「缩进 <= child_indent 且不是空/注释」的行处结束。
        let mut block_end = end;
        for probe in (index + 1)..end {
            let probe_raw = &lines[probe];
            if is_blank_or_comment(probe_raw) {
                continue;
            }
            if indentation(probe_raw)? <= child_indent {
                block_end = probe;
                break;
            }
        }
        let child = (index + 1..block_end)
            .find(|probe| !is_blank_or_comment(&lines[*probe]))
            .map(|probe| indentation(&lines[probe]).unwrap_or(child_indent + 2))
            .unwrap_or(child_indent + 2);
        return Ok(Some(Block {
            content_start: index + 1,
            end: block_end,
            child_indent: child,
        }));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// profile patch 的序列定位（0.2.0 布局）
//
// patch 文件顶层是 loader patch 序列（`- id: …`）。这里只提取定位所需的最低
// 信息——id、有没有 insert、块边界——**不解析 entry 的内容**：别的 entry 可能带
// `!!js` 表达式（dsh 的合法写法），不归启动器管，既不必读懂也一个字节不能动。
// ---------------------------------------------------------------------------

/// patch 顶层序列的一项。
#[derive(Debug, Clone)]
struct SeqEntry {
    /// entry 的 id（`id:` 的值，去掉成对引号）。提取不出时为 None——这种 entry
    /// 只参与边界与守卫，永远不会被当成 llm-pi-ai。
    id: Option<String>,
    /// 带 `insert` 的是插入清单条目，不是 config 的归属行（dsh 的 configEditor
    /// 找归属行时也跳过它）。
    has_insert: bool,
    /// `- ` 行的下标。
    #[allow(dead_code)]
    start: usize,
    /// entry 内容的起始行（= dash 行 + 1）。
    content_start: usize,
    /// 块结束行（不含）：下一个顶层 `- ` 行，或文件尾。
    end: usize,
    /// entry 字段的缩进（dsh 写出来是 2）。
    child_indent: usize,
}

/// 去掉成对引号。序列 entry 头里 id 的写法就这几种，不做完整的标量反转义。
fn unquote_simple(raw: &str) -> String {
    let text = raw.trim();
    if text.len() >= 2
        && ((text.starts_with('\'') && text.ends_with('\''))
            || (text.starts_with('"') && text.ends_with('"')))
    {
        return text[1..text.len() - 1].to_string();
    }
    text.to_string()
}

/// `- key: value` 行内写法里指定 key 的值文本（去引号）。dash 行不是这个 key、
/// 或值是块级（空）时返回 None。
fn dash_inline_value(line: &str, key: &str) -> Option<String> {
    let content = content_of(line);
    let after_dash = content.strip_prefix('-')?.trim_start();
    let (found, value) = split_key_value(after_dash)?;
    if found != key || value.is_empty() {
        return None;
    }
    Some(unquote_simple(&value))
}

/// entry 字段区里指定 key 的「行存在性 + 行内值」。只看不解析：`!!js` 带标签的
/// 值原样返回文本，不会因为读不懂而报错。
fn entry_field(lines: &[String], start: usize, end: usize, indent: usize, key: &str) -> Option<Option<String>> {
    for probe in start..end {
        let raw = &lines[probe];
        if is_blank_or_comment(raw) {
            continue;
        }
        if indentation(raw).ok()? != indent {
            continue;
        }
        let content = content_of(raw);
        let (found, value) = split_key_value(&content)?;
        if found != key {
            continue;
        }
        return Some(if value.is_empty() {
            None
        } else {
            Some(unquote_simple(&value))
        });
    }
    None
}

/// 把 patch 顶层切成序列项。顶层出现非 `- `、非 `[]` 的内容行说明这不是 dsh
/// 认识的 patch（dsh 自己也要求顶层是序列），直接报错而不是猜。
fn sequence_entries(lines: &[String]) -> Result<Vec<SeqEntry>, String> {
    let mut starts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if is_blank_or_comment(line) {
            continue;
        }
        if indentation(line)? != 0 {
            continue;
        }
        let content = content_of(line);
        // dsh 初始化 profile 时用 `[]` 占位空序列。
        if content == "[]" {
            continue;
        }
        if !content.starts_with('-') {
            return Err(format!(
                "profile patch 顶层第 {} 行不是 `- ` 序列项（`{content}`）。\
                 dsh 要求这份文件是 loader patch 序列，请手工修正后再试。",
                index + 1
            ));
        }
        starts.push(index);
    }
    let mut entries = Vec::new();
    for (position, &start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(lines.len());
        // 字段缩进：dash 行之后第一个非空行；entry 只有 dash 行时按 dsh 的 2 算。
        let child_indent = (start + 1..end)
            .find(|probe| !is_blank_or_comment(&lines[*probe]))
            .map(|probe| indentation(&lines[probe]))
            .transpose()?
            .filter(|indent| *indent > 0)
            .unwrap_or(2);
        // id 的两种写法：`- id: x` 行内，或 `-` 独占一行、字段区里写 `id: x`。
        let id = dash_inline_value(&lines[start], "id").or_else(|| {
            entry_field(lines, start + 1, end, child_indent, "id").flatten()
        });
        let has_insert = dash_inline_value(&lines[start], "insert").is_some()
            || entry_field(lines, start + 1, end, child_indent, "insert").is_some();
        entries.push(SeqEntry {
            id,
            has_insert,
            start,
            content_start: start + 1,
            end,
            child_indent,
        });
    }
    Ok(entries)
}

/// `providers` 字典在文件里的定位结果，含「还缺哪几层」。
#[derive(Debug, Clone)]
enum ProvidersSite {
    /// 两种布局各自的「连容器都没有」：旧布局缺 `llm-pi-ai:` 分区，新布局缺
    /// llm-pi-ai entry。插入时按布局补齐整个外层。
    MissingEntry,
    /// 仅新布局：有 entry 但没有 `config:` 块。记录 entry 的内容边界与字段缩进。
    MissingConfig {
        content_start: usize,
        end: usize,
        child_indent: usize,
    },
    /// 旧布局：有分区没有 `providers:`；新布局：有 `config:` 没有 `providers:`。
    /// 记录待补 `providers:` 的那个块的内容边界与子缩进。
    MissingProviders {
        content_start: usize,
        end: usize,
        child_indent: usize,
    },
    /// 完整定位：`providers:` 的内容边界与供应方 key 的缩进。
    Present {
        content_start: usize,
        end: usize,
        child_indent: usize,
    },
}

/// 定位 `providers` 字典。新布局与 dsh 的 configEditor 一致：取**最后一个**
/// 不带 `insert` 的 llm-pi-ai entry（findLastIndex 语义）。
fn resolve_providers_site(lines: &[String], layout: DshSettingsLayout) -> Result<ProvidersSite, String> {
    let total = lines.len();
    match layout {
        DshSettingsLayout::LegacyYaml => {
            let Some(section) = find_block(lines, 0, total, 0, PI_AI_SECTION)? else {
                return Ok(ProvidersSite::MissingEntry);
            };
            let Some(providers) = find_block(
                lines,
                section.content_start,
                section.end,
                section.child_indent,
                PROVIDERS_KEY,
            )?
            else {
                return Ok(ProvidersSite::MissingProviders {
                    content_start: section.content_start,
                    end: section.end,
                    child_indent: section.child_indent,
                });
            };
            Ok(ProvidersSite::Present {
                content_start: providers.content_start,
                end: providers.end,
                child_indent: providers.child_indent,
            })
        }
        DshSettingsLayout::ProfilePatch => {
            let entries = sequence_entries(lines)?;
            let Some(entry) = entries
                .iter()
                .rev()
                .find(|entry| entry.id.as_deref() == Some(PI_AI_SECTION) && !entry.has_insert)
            else {
                return Ok(ProvidersSite::MissingEntry);
            };
            let Some(config) = find_block(
                lines,
                entry.content_start,
                entry.end,
                entry.child_indent,
                CONFIG_KEY,
            )?
            else {
                return Ok(ProvidersSite::MissingConfig {
                    content_start: entry.content_start,
                    end: entry.end,
                    child_indent: entry.child_indent,
                });
            };
            let Some(providers) = find_block(
                lines,
                config.content_start,
                config.end,
                config.child_indent,
                PROVIDERS_KEY,
            )?
            else {
                return Ok(ProvidersSite::MissingProviders {
                    content_start: config.content_start,
                    end: config.end,
                    child_indent: config.child_indent,
                });
            };
            Ok(ProvidersSite::Present {
                content_start: providers.content_start,
                end: providers.end,
                child_indent: providers.child_indent,
            })
        }
    }
}

/// 「写入没把文件改瘦」的守卫。旧布局查顶层分区名一个不少；新布局查顶层
/// entry 数不减少、且每个有 id 的 entry 都还在（无名 entry 由数量兜住）。
fn assert_top_level_kept(
    before: &[String],
    after: &[String],
    layout: DshSettingsLayout,
) -> Result<(), String> {
    match layout {
        DshSettingsLayout::LegacyYaml => {
            let before_keys = top_level_keys(before);
            let after_keys = top_level_keys(after);
            for key in &before_keys {
                if !after_keys.contains(key) {
                    return Err(format!(
                        "写入结果丢失了顶层分区 `{key}`，已中止写入（原文件未被修改）。\
                         这通常意味着设置文件里有启动器不认识的写法，请手动编辑或反馈。"
                    ));
                }
            }
            Ok(())
        }
        DshSettingsLayout::ProfilePatch => {
            let before_entries = sequence_entries(before)?;
            let after_entries = sequence_entries(after)?;
            if after_entries.len() < before_entries.len() {
                return Err(
                    "写入结果让 profile patch 的顶层 entry 变少了，已中止写入（原文件未被修改）。"
                        .to_string(),
                );
            }
            let after_ids: Vec<&str> = after_entries
                .iter()
                .filter_map(|entry| entry.id.as_deref())
                .collect();
            for entry in &before_entries {
                if let Some(id) = entry.id.as_deref() {
                    if !after_ids.contains(&id) {
                        return Err(format!(
                            "写入结果丢失了 profile patch 的顶层 entry `{id}`，已中止写入\
                             （原文件未被修改）。这通常意味着文件里有启动器不认识的写法，\
                             请手动编辑或反馈。"
                        ));
                    }
                }
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// 读取
// ---------------------------------------------------------------------------

fn text_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect()
}

/// 解析 `llm-pi-ai.providers` 下的全部供应方（两种布局共用——定位差异都在
/// [`resolve_providers_site`] 里，块内解析是同一份）。
fn parse_providers(lines: &[String], layout: DshSettingsLayout) -> Result<Vec<DshProviderProfile>, String> {
    let ProvidersSite::Present {
        content_start,
        end,
        child_indent,
    } = resolve_providers_site(lines, layout)?
    else {
        return Ok(Vec::new());
    };

    let items = field_items(lines, content_start, end, child_indent)?;
    let mut result = Vec::new();
    for item in items {
        result.push(parse_provider(lines, &item)?);
    }
    Ok(result)
}

fn parse_provider(lines: &[String], item: &FieldItem) -> Result<DshProviderProfile, String> {
    let mut profile = DshProviderProfile {
        id: item.key.clone(),
        ..Default::default()
    };
    // 子内容缩进：key 行之后的第一个非空行。
    let child_indent = (item.start + 1..item.end)
        .find(|probe| !is_blank_or_comment(&lines[*probe]))
        .map(|probe| indentation(&lines[probe]))
        .transpose()?
        .unwrap_or(indentation(&lines[item.start])? + 2);

    let fields = field_items(lines, item.start + 1, item.end, child_indent)?;
    for field in &fields {
        match field.key.as_str() {
            "displayName" => profile.display_name = field.value.as_text().filter(|v| !v.is_empty()),
            "api" => profile.api = field.value.as_text().filter(|v| !v.is_empty()),
            "baseURL" => profile.base_url = field.value.as_text().filter(|v| !v.is_empty()),
            "apiKeyEnv" => profile.api_key_env = field.value.as_text().filter(|v| !v.is_empty()),
            "reasoning" => profile.reasoning = field.value.as_text().filter(|v| !v.is_empty()),
            "models" => profile.models = parse_models(lines, field)?,
            _ => {}
        }
        if !MANAGED_PROVIDER_KEYS.contains(&field.key.as_str()) {
            profile.unmanaged_fields.push(field.key.clone());
        }
    }
    Ok(profile)
}

/// 解析 `models:` 下的序列。支持 `- id: x` 起头、后续行同缩进的常见写法。
fn parse_models(lines: &[String], item: &FieldItem) -> Result<Vec<DshModelProfile>, String> {
    // 序列项（`- `）的缩进：`models:` 之后的第一个非空行。
    let Some(first) = (item.start + 1..item.end).find(|probe| !is_blank_or_comment(&lines[*probe]))
    else {
        return Ok(Vec::new());
    };
    let dash_indent = indentation(&lines[first])?;
    if !lines[first].trim_start().starts_with('-') {
        return Err(
            "设置文件里的 `models:` 不是序列（没有以 `-` 开头的条目），启动器无法解析。"
                .to_string(),
        );
    }

    // 收集每个序列项的起始行。
    let mut starts: Vec<usize> = Vec::new();
    for probe in item.start + 1..item.end {
        let raw = &lines[probe];
        if is_blank_or_comment(raw) {
            continue;
        }
        if indentation(raw)? != dash_indent {
            continue;
        }
        if raw.trim_start().starts_with('-') {
            starts.push(probe);
        }
    }

    let mut models = Vec::new();
    for (position, &start) in starts.iter().enumerate() {
        let end = starts.get(position + 1).copied().unwrap_or(item.end);
        models.push(parse_model_entry(lines, start, end, dash_indent)?);
    }
    Ok(models)
}

fn parse_model_entry(
    lines: &[String],
    start: usize,
    end: usize,
    dash_indent: usize,
) -> Result<DshModelProfile, String> {
    // `- id: x` 里的键，虚拟地当作缩进 dash_indent + 2 的一行；后续同缩进的行
    // 也当作它的同级字段。
    let first_content = content_of(&lines[start]);
    let inline = first_content
        .trim_start()
        .strip_prefix('-')
        .unwrap_or("")
        .trim()
        .to_string();
    let field_indent = dash_indent + 2;

    let mut model = DshModelProfile::default();
    if !inline.is_empty() {
        let Some((key, value)) = split_key_value(&inline) else {
            return Err(format!(
                "设置文件里的模型条目 `{first_content}` 不是 `- 键: 值` 形式，无法解析。"
            ));
        };
        apply_model_field(&mut model, &key, &parse_scalar(&value)?)?;
        if !MANAGED_MODEL_KEYS.contains(&key.as_str()) {
            model.unmanaged_fields.push(key);
        }
    }

    let fields = field_items(lines, start + 1, end, field_indent)?;
    for field in &fields {
        apply_model_field(&mut model, &field.key, &field.value)?;
        // reasoningEfforts 是嵌套映射，需要再看一层。
        if field.key == "reasoningEfforts" && matches!(field.value, Scalar::Empty) {
            model.reasoning_efforts = parse_reasoning_efforts(lines, field)?;
        }
        if !MANAGED_MODEL_KEYS.contains(&field.key.as_str())
            && !model.unmanaged_fields.contains(&field.key)
        {
            model.unmanaged_fields.push(field.key.clone());
        }
    }
    Ok(model)
}

fn apply_model_field(
    model: &mut DshModelProfile,
    key: &str,
    value: &Scalar,
) -> Result<(), String> {
    match key {
        "id" => model.id = value.as_text().unwrap_or_default(),
        "name" => model.name = value.as_text().filter(|v| !v.is_empty()),
        "contextWindow" => model.context_window = parse_count(value, "contextWindow")?,
        "maxTokens" => model.max_tokens = parse_count(value, "maxTokens")?,
        "input" => model.input = parse_modalities(value)?,
        "reasoningEfforts" => match value {
            Scalar::Text(text) if text == "false" => model.reasoning_disabled = true,
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn parse_count(value: &Scalar, key: &str) -> Result<Option<u64>, String> {
    match value.as_text() {
        None => Ok(None),
        Some(text) => text
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("设置文件里的 `{key}: {text}` 不是整数，无法解析。")),
    }
}

/// `input: [text, image]` 与块序列两种写法都要认。
fn parse_modalities(value: &Scalar) -> Result<Option<Vec<String>>, String> {
    match value {
        Scalar::Empty => Ok(None),
        Scalar::Text(text) => Ok(Some(vec![text.trim().to_string()])),
        Scalar::Flow(text) => {
            let inner = text
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            let items = inner
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            Ok(Some(items))
        }
    }
}

fn parse_reasoning_efforts(
    lines: &[String],
    item: &FieldItem,
) -> Result<Vec<DshReasoningLevel>, String> {
    let Some(first) = (item.start + 1..item.end).find(|probe| !is_blank_or_comment(&lines[*probe]))
    else {
        return Ok(Vec::new());
    };
    let child_indent = indentation(&lines[first])?;
    let fields = field_items(lines, item.start + 1, item.end, child_indent)?;
    Ok(fields
        .into_iter()
        .map(|field| DshReasoningLevel {
            level: field.key,
            wire: field.value.as_text().filter(|value| !value.is_empty()),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// YAML 保留字与「看起来像别的类型」的标量：这些必须加引号，否则会被读成
/// 布尔、空值或数字。
fn needs_quotes(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let lowered = value.to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
    ) {
        return true;
    }
    if value.parse::<f64>().is_ok() {
        return true;
    }
    if value.starts_with(' ') || value.ends_with(' ') {
        return true;
    }
    let risky_prefix = [
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ];
    if value.starts_with(|ch: char| risky_prefix.contains(&ch)) {
        return true;
    }
    // 冒号**加空格**才是映射分隔符（`a: b` 会被读成嵌套映射）；URL 里的单个冒号
    // 不是，`baseURL: https://x/v1` 是合法的普通标量，不该被引号包起来。
    if value.contains(": ") || value.ends_with(':') {
        return true;
    }
    // 空白之后的 `#` 会起始注释，把值从那里截断。
    if value.contains(" #") {
        return true;
    }
    value.contains('\n') || value.contains('\t')
}

fn render_scalar(value: &str) -> String {
    if needs_quotes(value) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_string()
    }
}

fn render_model(model: &DshModelProfile, dash_indent: usize) -> Vec<String> {
    let dash_pad = " ".repeat(dash_indent);
    let field_pad = " ".repeat(dash_indent + 2);
    let deep_pad = " ".repeat(dash_indent + 4);

    let mut lines = vec![format!("{dash_pad}- id: {}", render_scalar(&model.id))];
    if let Some(name) = model.name.as_deref().filter(|name| !name.is_empty()) {
        lines.push(format!("{field_pad}name: {}", render_scalar(name)));
    }
    if let Some(context) = model.context_window {
        lines.push(format!("{field_pad}contextWindow: {context}"));
    }
    if let Some(max_tokens) = model.max_tokens {
        lines.push(format!("{field_pad}maxTokens: {max_tokens}"));
    }
    if let Some(modalities) = model.input.as_ref().filter(|items| !items.is_empty()) {
        let joined = modalities
            .iter()
            .map(|item| render_scalar(item))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{field_pad}input: [{joined}]"));
    }
    if model.reasoning_disabled {
        lines.push(format!("{field_pad}reasoningEfforts: false"));
    } else if !model.reasoning_efforts.is_empty() {
        lines.push(format!("{field_pad}reasoningEfforts:"));
        for level in &model.reasoning_efforts {
            match level.wire.as_deref().filter(|wire| !wire.is_empty()) {
                Some(wire) => lines.push(format!("{deep_pad}{}: {}", level.level, render_scalar(wire))),
                // 只有 off 允许留空：值是「支持这一档，但不发任何参数」。
                None => lines.push(format!("{deep_pad}{}:", level.level)),
            }
        }
    }
    lines
}

fn render_provider(provider: &DshProviderProfile, indent: usize) -> Vec<String> {
    let pad = " ".repeat(indent);
    let field_pad = " ".repeat(indent + 2);
    let mut lines = vec![format!("{pad}{}:", provider.id)];
    if let Some(api_key_env) = provider.api_key_env.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("{field_pad}apiKeyEnv: {}", render_scalar(api_key_env)));
    }
    if let Some(display_name) = provider.display_name.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!(
            "{field_pad}displayName: {}",
            render_scalar(display_name)
        ));
    }
    if let Some(api) = provider.api.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("{field_pad}api: {}", render_scalar(api)));
    }
    if let Some(base_url) = provider.base_url.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("{field_pad}baseURL: {}", render_scalar(base_url)));
    }
    if let Some(reasoning) = provider.reasoning.as_deref().filter(|value| !value.is_empty()) {
        lines.push(format!("{field_pad}reasoning: {}", render_scalar(reasoning)));
    }
    if !provider.models.is_empty() {
        lines.push(format!("{field_pad}models:"));
        for model in &provider.models {
            lines.extend(render_model(model, indent + 4));
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/// 校验一个档位声明。规则与 dsh 的 `resolveModelReasoning` 一致，目的是在写盘
/// **之前**就把 dsh 会拒绝的东西挡下来，而不是让用户启动时才发现。
fn validate_reasoning_efforts(model: &DshModelProfile) -> Result<(), String> {
    if model.reasoning_disabled {
        if !model.reasoning_efforts.is_empty() {
            return Err(format!(
                "模型 `{}` 同时声明了 `reasoningEfforts: false` 和具体档位，只能二选一。",
                model.id
            ));
        }
        return Ok(());
    }
    if model.reasoning_efforts.is_empty() {
        return Ok(());
    }
    for level in &model.reasoning_efforts {
        if !THINKING_LEVELS.contains(&level.level.as_str()) {
            return Err(format!(
                "模型 `{}` 的思考档位 `{}` 不是 dsh 认识的档位（可用：{}）。",
                model.id,
                level.level,
                THINKING_LEVELS.join(" / ")
            ));
        }
        match level.wire.as_deref() {
            None => {
                if level.level != "off" {
                    return Err(format!(
                        "模型 `{}` 的档位 `{}` 缺少 wire 值——只有 `off` 可以留空（表示不发送参数）。",
                        model.id, level.level
                    ));
                }
            }
            Some(wire) => {
                if wire.trim().is_empty() {
                    return Err(format!("模型 `{}` 的档位 `{}` 的 wire 值不能是空字符串。", model.id, level.level));
                }
            }
        }
    }
    if !model.reasoning_efforts.iter().any(|level| level.level != "off") {
        return Err(format!(
            "模型 `{}` 只声明了 `off`，dsh 要求至少声明一个真正的思考档位；\
             若这个模型不推理，请改用「非推理模型」。",
            model.id
        ));
    }
    Ok(())
}

/// 凭据引用名是否合法：`^[A-Za-z_][A-Za-z0-9_]*$`，即 POSIX shell 变量名。
///
/// 这不是我们自己挑的规矩：dsh 解析 profile 时直接对它调用
/// `credentialRef()`（`dsh-llm-pi-ai` → `@deepseek-ai/dsh-credentials` 的
/// `REF_PATTERN`），不合法会**抛错**而不会降级——连字符、点、空格都会让它
/// 整条路由起不来。所以在这里先拦下来，别让它写进文件。
fn is_credential_ref_name(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn validate_provider(provider: &DshProviderProfile) -> Result<(), String> {
    if provider.id.trim().is_empty() {
        return Err("供应方 ID 不能为空。".to_string());
    }
    // dsh 的路由键必须是能作为凭据记录键的标识符（小写字母、数字、连字符）。
    if !provider
        .id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!(
            "供应方 ID `{}` 只能用小写字母、数字和连字符——dsh 用它派生凭据记录键。",
            provider.id
        ));
    }
    // 留空是合法的：表示「已配置但无密钥」，对目录路由而言即交给 pi-ai 自己的环境发现。
    if let Some(reference) = provider
        .api_key_env
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        if !is_credential_ref_name(reference) {
            return Err(format!(
                "凭据引用名 `{reference}` 不合法：只能用字母、数字和下划线，且不能以数字开头。\
                 dsh 的「设置 → 模型」页按同一条规则派生键名（路由键转大写，连字符变成下划线）。"
            ));
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for model in &provider.models {
        if model.id.trim().is_empty() {
            return Err(format!("供应方 `{}` 下有模型条目缺少 ID。", provider.id));
        }
        if seen.contains(&model.id.as_str()) {
            return Err(format!("供应方 `{}` 下模型 ID `{}` 重复。", provider.id, model.id));
        }
        seen.push(model.id.as_str());
        validate_reasoning_efforts(model)?;
    }
    Ok(())
}

/// 顶层键的集合，用于「写入没有把文件改瘦」的兜底检查。
fn top_level_keys(lines: &[String]) -> Vec<String> {
    let mut keys = Vec::new();
    for line in lines {
        if is_blank_or_comment(line) {
            continue;
        }
        if indentation(line).unwrap_or(1) != 0 {
            continue;
        }
        if let Some((key, _)) = split_key_value(&content_of(line)) {
            keys.push(key);
        }
    }
    keys
}

// ---------------------------------------------------------------------------
// 写入
// ---------------------------------------------------------------------------

/// 就地套用一组字段更新。
///
/// 关键约束：**只碰启动器负责的字段**。供应商块里其它字段（`compat` /
/// `headers` / `retryPolicy` …）不在 `updates` 里，因此原样留在原处，注释也不受影响。
///
/// 更新按「文件里靠后的字段先处理」的顺序套用，这样前面字段行号的移动不会让
/// 后面还没处理的字段定位失效。
fn apply_field_updates(
    lines: &mut Vec<String>,
    item: &FieldItem,
    child_indent: usize,
    updates: Vec<(String, Option<Vec<String>>)>,
) -> Result<(), String> {
    // 所有改动都落在 [start, end) 内，所以下界不动，上界要跟着行数变化走。
    //
    // 这一点非做不可：同一个 `providers` 字典下，**每个供应商的字段缩进都一样**，
    // 所以一个过期的上界会把查找范围伸进下一个供应商，而 `find` 取的是首个匹配
    // ——于是「本供应商缺某个字段」会被误判成「下一个供应商有」，直接改坏别人的
    // 配置。块内改动的行数差就是这个偏移量。
    let start = item.start + 1;
    let mut end = item.end;

    for (key, rendered) in updates {
        let existing = {
            let items = field_items(lines, start, end, child_indent)?;
            items
                .iter()
                .find(|candidate| candidate.key == key)
                .map(|found| (found.start, found.content_end(lines)))
        };
        let before = lines.len();
        match (existing, rendered) {
            (Some((field_start, field_end)), Some(new_lines)) => {
                lines.splice(field_start..field_end, new_lines);
            }
            (Some((field_start, field_end)), None) => {
                lines.drain(field_start..field_end);
            }
            (None, Some(new_lines)) => {
                let insert_at = append_index(lines, start, end);
                lines.splice(insert_at..insert_at, new_lines);
            }
            (None, None) => {}
        }
        end = (end as isize + (lines.len() as isize - before as isize)) as usize;
    }
    Ok(())
}

fn provider_field_updates(provider: &DshProviderProfile, child_indent: usize) -> Vec<(String, Option<Vec<String>>)> {
    let field_pad = " ".repeat(child_indent);
    let text = |key: &str, value: Option<&String>| -> (String, Option<Vec<String>>) {
        let rendered = value
            .filter(|value| !value.trim().is_empty())
            .map(|value| vec![format!("{field_pad}{key}: {}", render_scalar(value))]);
        (key.to_string(), rendered)
    };

    let mut updates = vec![
        text("apiKeyEnv", provider.api_key_env.as_ref()),
        text("displayName", provider.display_name.as_ref()),
        text("api", provider.api.as_ref()),
        text("baseURL", provider.base_url.as_ref()),
        text("reasoning", provider.reasoning.as_ref()),
    ];

    let models = if provider.models.is_empty() {
        None
    } else {
        let mut rendered = vec![format!("{field_pad}models:")];
        for model in &provider.models {
            rendered.extend(render_model(model, child_indent + 2));
        }
        Some(rendered)
    };
    updates.push(("models".to_string(), models));
    updates
}

/// 把一个新的供应方块插进 `providers:` 里；外层容器（旧布局的分区/新布局的
/// entry 与 config）缺哪层补哪层。
fn insert_provider(
    lines: &mut Vec<String>,
    provider: &DshProviderProfile,
    layout: DshSettingsLayout,
) -> Result<(), String> {
    insert_provider_with(lines, layout, |indent| render_provider(provider, indent))
}

/// 插入一个供应商的**已渲染**块。`render` 接收供应商 key 的目标缩进，返回整块
/// 的行——写入路径渲染结构化档案，恢复路径（[`restore_imported_from_text`]）
/// 平移残留文件里的原始块；落位规则（缺哪层补哪层、落点跳注释）两条路共用。
fn insert_provider_with(
    lines: &mut Vec<String>,
    layout: DshSettingsLayout,
    render: impl FnOnce(usize) -> Vec<String>,
) -> Result<(), String> {
    match resolve_providers_site(lines, layout)? {
        ProvidersSite::Present {
            content_start,
            end,
            child_indent,
        } => {
            let rendered = render(child_indent);
            let insert_at = append_index(lines, content_start, end);
            lines.splice(insert_at..insert_at, rendered);
        }
        ProvidersSite::MissingProviders {
            content_start,
            end,
            child_indent,
        } => {
            let pad = " ".repeat(child_indent);
            let mut rendered = vec![format!("{pad}{PROVIDERS_KEY}:")];
            rendered.extend(render(child_indent + 2));
            let insert_at = append_index(lines, content_start, end);
            lines.splice(insert_at..insert_at, rendered);
        }
        ProvidersSite::MissingConfig {
            content_start,
            end,
            child_indent,
        } => {
            // 只有 patch 布局会走到这里：entry 还在，补 config 与 providers 两层。
            let pad = " ".repeat(child_indent);
            let mut rendered = vec![
                format!("{pad}{CONFIG_KEY}:"),
                format!("{pad}  {PROVIDERS_KEY}:"),
            ];
            rendered.extend(render(child_indent + 4));
            let insert_at = append_index(lines, content_start, end);
            lines.splice(insert_at..insert_at, rendered);
        }
        ProvidersSite::MissingEntry => match layout {
            DshSettingsLayout::LegacyYaml => {
                // 整个分区都不存在：在文件头补齐。dsh 对顶层顺序没有要求。
                let mut rendered = vec![format!("{PI_AI_SECTION}:")];
                rendered.push(format!("  {PROVIDERS_KEY}:"));
                rendered.extend(render(4));
                rendered.push(String::new());
                lines.splice(0..0, rendered);
            }
            DshSettingsLayout::ProfilePatch => {
                // 连 entry 都没有：在文件尾追加一整条，形状照 dsh 自己的 configEditor
                // 写出来的样子（id + name + config.providers）。dsh 初始化 profile
                // 时用 `[]` 占位空序列——追加前把它收走，否则两份内容拼不出合法 YAML。
                lines.retain(|line| !(indentation(line).unwrap_or(1) == 0 && content_of(line) == "[]"));
                let mut rendered = vec![
                    format!("- id: {PI_AI_SECTION}"),
                    format!("  name: {}", render_scalar(PI_AI_ENTRY_NAME)),
                    format!("  {CONFIG_KEY}:"),
                    format!("    {PROVIDERS_KEY}:"),
                ];
                rendered.extend(render(6));
                // 落点：文件尾最后一个非空行之后。这里**不能**用 append_index——
                // 它会跳过尾部注释，而顶层的尾部注释是文件头/文件级注释（比如 dsh
                // 写的那段说明），被挤到新 entry 后面就错了。块级插入才跳注释。
                let mut insert_at = lines.len();
                while insert_at > 0 && lines[insert_at - 1].trim().is_empty() {
                    insert_at -= 1;
                }
                lines.splice(insert_at..insert_at, rendered);
            }
        },
    }
    Ok(())
}

/// 删掉一个已存在的供应方块，连它前面的注释行一起（那些注释描述的就是它）。
fn remove_provider(
    lines: &mut Vec<String>,
    provider_id: &str,
    layout: DshSettingsLayout,
) -> Result<bool, String> {
    let ProvidersSite::Present {
        content_start,
        end,
        child_indent,
    } = resolve_providers_site(lines, layout)?
    else {
        return Ok(false);
    };
    let items = field_items(lines, content_start, end, child_indent)?;
    let Some(item) = items.iter().find(|item| item.key == provider_id) else {
        return Ok(false);
    };
    let mut start = item.start;
    while start > content_start && is_blank_or_comment(&lines[start - 1]) {
        start -= 1;
    }
    lines.drain(start..item.end);
    Ok(true)
}

/// 块内追加内容时的落点：跳过尾部的空行与纯注释行。
///
/// 文件通常以换行结尾，`split('\n')` 会多出一个空行；直接追加到块尾就会把它留在
/// 新增内容之前（看起来像凭空多了一个空行）。收尾注释同理——它描述的是块本身，
/// 不该被挤到新增内容之后。
fn append_index(lines: &[String], start: usize, end: usize) -> usize {
    let mut index = end;
    while index > start && is_blank_or_comment(&lines[index - 1]) {
        index -= 1;
    }
    index
}

/// 定位一个已存在的供应方块，返回它所在字典的子缩进（供应方 key 的缩进）与该项。
fn locate_provider(
    lines: &[String],
    provider_id: &str,
    layout: DshSettingsLayout,
) -> Result<Option<(usize, FieldItem)>, String> {
    let ProvidersSite::Present {
        content_start,
        end,
        child_indent,
    } = resolve_providers_site(lines, layout)?
    else {
        return Ok(None);
    };
    let items = field_items(lines, content_start, end, child_indent)?;
    Ok(items
        .into_iter()
        .find(|item| item.key == provider_id)
        .map(|item| (child_indent, item)))
}

fn join_lines(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// 一次性导入残留（settings.yaml.imported）的检测与恢复
//
// dsh 0.2.0 的 importLegacyDocument 先改名、再逐节导入；被装配校验拒绝的节
// （典型：自定义路由缺 api）只留在 .imported 里，界面上无声无息。恢复走
// **原始块移植**而不是结构化重渲染：供应商块里启动器不编辑的字段（headers /
// compat / retryPolicy …）与注释都逐字节带回，只把缩进平移到目标布局的深度。
// ---------------------------------------------------------------------------

/// 恢复前必须清零的问题清单（空 = 可恢复）。patch 布局下叠加 dsh 0.2.0 对
/// 自定义路由的硬性要求——那正是当初导入被拒的原因，原样恢复回去会再丢一次。
fn import_problems(provider: &DshProviderProfile, target_layout: DshSettingsLayout) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(reason) = validate_provider(provider) {
        problems.push(reason);
    }
    if target_layout == DshSettingsLayout::ProfilePatch {
        if let Err(reason) = enforce_patch_route_rules(provider) {
            problems.push(reason);
        }
    }
    problems
}

/// 从残留文件文本里挑出「当前文档里没有」的供应商（纯文本，方便测试）。
fn scan_imported_text(
    existing_ids: &std::collections::HashSet<&str>,
    imported_text: &str,
    target_layout: DshSettingsLayout,
) -> Result<Vec<DshImportedProvider>, String> {
    let imported_lines = text_lines(imported_text);
    let mut imported = parse_providers(&imported_lines, DshSettingsLayout::LegacyYaml)?;
    mark_custom_routes(&mut imported, target_layout);
    Ok(imported
        .into_iter()
        .filter(|provider| !existing_ids.contains(provider.id.as_str()))
        .map(|provider| {
            let problems = import_problems(&provider, target_layout);
            DshImportedProvider { provider, problems }
        })
        .collect())
}

/// 读取残留文件并扫描（读取命令的副作用位）。文件不存在 → None；读不了/解析
/// 失败 → 仍有返回值，原因放在 parse_error 里让界面能说清，而不是静默吃掉。
fn scan_legacy_import(
    providers: &[DshProviderProfile],
    layout: DshSettingsLayout,
) -> Option<DshLegacyImport> {
    let path = dsh_imported_settings_path().ok()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(DshLegacyImport {
                path: path.display().to_string(),
                parse_error: Some(format!("无法读取导入残留文件：{error}")),
                candidates: Vec::new(),
            });
        }
    };
    let existing: std::collections::HashSet<&str> =
        providers.iter().map(|provider| provider.id.as_str()).collect();
    let path_display = path.display().to_string();
    match scan_imported_text(&existing, &text, layout) {
        Ok(candidates) if candidates.is_empty() => None,
        Ok(candidates) => Some(DshLegacyImport {
            path: path_display,
            parse_error: None,
            candidates,
        }),
        Err(reason) => Some(DshLegacyImport {
            path: path_display,
            parse_error: Some(reason),
            candidates: Vec::new(),
        }),
    }
}

/// 取出残留文件里一个供应商的原始块：连块前的注释行一起（它们描述的就是它），
/// 尾部空行/注释裁掉（那属于下一块的头部）。返回供应商 key 的缩进与行切片。
fn provider_raw_block(
    lines: &[String],
    provider_id: &str,
) -> Result<Option<(usize, Vec<String>)>, String> {
    let ProvidersSite::Present {
        content_start,
        end,
        child_indent,
    } = resolve_providers_site(lines, DshSettingsLayout::LegacyYaml)?
    else {
        return Ok(None);
    };
    let items = field_items(lines, content_start, end, child_indent)?;
    let Some(item) = items.iter().find(|item| item.key == provider_id) else {
        return Ok(None);
    };
    let mut start = item.start;
    while start > content_start && is_blank_or_comment(&lines[start - 1]) {
        start -= 1;
    }
    let mut block_end = item.end;
    while block_end > start + 1 && is_blank_or_comment(&lines[block_end - 1]) {
        block_end -= 1;
    }
    Ok(Some((child_indent, lines[start..block_end].to_vec())))
}

/// 把取出的块从源缩进平移到目标缩进。空行原样；非空行加/减前导空格
/// （减的时候以该行实际缩进为上限，不会裁到内容）。
fn shift_block_lines(block: &[String], from_indent: usize, to_indent: usize) -> Vec<String> {
    block
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return line.clone();
            }
            if to_indent >= from_indent {
                format!("{}{}", " ".repeat(to_indent - from_indent), line)
            } else {
                let leading = line.len() - line.trim_start().len();
                let drop = (from_indent - to_indent).min(leading);
                line[drop..].to_string()
            }
        })
        .collect()
}

/// 恢复的核心（纯文本，方便测试）：把残留文件里当前文档缺失的供应商原始块
/// 移植进当前文档文本。当前文档可以是空串（文件还不存在——插入路径会补层）。
fn restore_imported_from_text(
    original: &str,
    imported_text: &str,
    layout: DshSettingsLayout,
) -> Result<(String, Vec<String>, Vec<DshImportedProvider>), String> {
    let mut lines = text_lines(original);
    let before_lines = lines.clone();
    let imported_lines = text_lines(imported_text);
    let imported = parse_providers(&imported_lines, DshSettingsLayout::LegacyYaml)?;
    let current = parse_providers(&lines, layout)?;
    let current_ids: std::collections::HashSet<&str> =
        current.iter().map(|provider| provider.id.as_str()).collect();

    let mut restored: Vec<String> = Vec::new();
    let mut skipped: Vec<DshImportedProvider> = Vec::new();
    for provider in imported {
        if current_ids.contains(provider.id.as_str()) {
            continue;
        }
        let problems = import_problems(&provider, layout);
        if !problems.is_empty() {
            skipped.push(DshImportedProvider { provider, problems });
            continue;
        }
        let Some((source_indent, block)) = provider_raw_block(&imported_lines, &provider.id)?
        else {
            skipped.push(DshImportedProvider {
                provider,
                problems: vec!["在残留文件里定位不到它的配置块。".to_string()],
            });
            continue;
        };
        insert_provider_with(&mut lines, layout, |indent| {
            shift_block_lines(&block, source_indent, indent)
        })?;
        restored.push(provider.id.clone());
    }
    let updated = join_lines(&lines);
    assert_top_level_kept(&before_lines, &lines, layout)?;
    Ok((updated, restored, skipped))
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 读取当前布局的设置文件。布局由检测到的 dsh 版本决定（见 [`settings_layout`]）。
fn read_document() -> Result<(PathBuf, String, bool, DshSettingsLayout), String> {
    let layout = settings_layout();
    let path = document_path(layout)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((path, text, true, layout)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((path, String::new(), false, layout))
        }
        Err(error) => Err(format!("无法读取 dsh 设置文件 {}：{error}", path.display())),
    }
}

/// 写入结果的备份路径，与 `file_transaction` 的 sidecar 约定一致（`<原名>.bak`）。
fn backup_path_of(path: &std::path::Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.bak"))
        .unwrap_or_else(|| "bak".to_string());
    path.with_extension(extension)
}

/// 读取设置文件里的供应方与模型。
#[tauri::command]
pub fn dsh_read_settings() -> Result<DshSettingsDocument, String> {
    let (path, text, exists, layout) = read_document()?;
    let lines = text_lines(&text);
    let mut providers = if exists {
        parse_providers(&lines, layout)?
    } else {
        Vec::new()
    };
    mark_custom_routes(&mut providers, layout);
    let legacy_import = scan_legacy_import(&providers, layout);
    Ok(DshSettingsDocument {
        path: path.display().to_string(),
        exists,
        revision: revision_of(&text),
        supported_version: layout.supported_version().to_string(),
        layout,
        providers,
        legacy_import,
    })
}

/// 0.2.0 起 dsh 装配 llm-pi-ai 时对**自定义路由**的硬性要求（`resolveEntry`）：
/// api、baseURL、至少一个模型，缺一个就抛 `PiAiCatalogError`，整条 llm-pi-ai
/// 配置被拒——包括启动时的一次性导入在内——且只在日志里留一句 warn，界面上
/// 无声无息（2026-10-05「strata 导入后消失」的根因）。写盘前就在这里挡住，
/// 错误信息直接说人话。
///
/// 目录路由不查：缺的字段有内置目录兜底。目录清单读不到时也不替 dsh 执法
/// （判不准自定义与否，误伤比漏过更糟）。
fn enforce_patch_route_rules(provider: &DshProviderProfile) -> Result<(), String> {
    let Some(catalog) = installed_catalog_routes(DshSettingsLayout::ProfilePatch) else {
        return Ok(());
    };
    if catalog.contains(&provider.id) {
        return Ok(());
    }
    if provider.api.as_deref().is_none_or(|value| value.trim().is_empty()) {
        return Err(format!(
            "供应方 `{}` 是自定义路由，dsh 0.2.0 起必须选择 wire 协议\
             （openai-completions / openai-responses / anthropic-messages）。\
             缺少协议的自定义路由会让 dsh 拒绝整个 llm-pi-ai 配置。",
            provider.id
        ));
    }
    if provider.base_url.as_deref().is_none_or(|value| value.trim().is_empty()) {
        return Err(format!(
            "供应方 `{}` 是自定义路由，dsh 0.2.0 起必须填写 API 地址（baseURL）。",
            provider.id
        ));
    }
    if provider.models.is_empty() {
        return Err(format!(
            "供应方 `{}` 是自定义路由，dsh 0.2.0 起至少要声明一个模型——\
             内置目录里没有它的模型可兜底。",
            provider.id
        ));
    }
    Ok(())
}

/// 写入（新增或更新）一个供应方。只动这一个块，其余内容逐字节保留。
#[tauri::command]
pub fn dsh_write_provider(
    request: DshWriteProviderRequest,
) -> Result<DshSettingsWriteResult, String> {
    validate_provider(&request.provider)?;

    let (path, original, _, layout) = read_document()?;
    if revision_of(&original) != request.base_revision {
        return Err(
            "dsh 设置文件在读取之后被其它程序改过了，为避免覆盖对方的改动，本次写入已取消。\
             请重新读取后再试。"
                .to_string(),
        );
    }
    if layout == DshSettingsLayout::ProfilePatch {
        enforce_patch_route_rules(&request.provider)?;
    }

    let mut lines = text_lines(&original);
    let before_lines = lines.clone();

    // 改名 = 先删旧块再按新增插入。同名更新走就地改字段，这样启动器不负责的
    // 字段（`compat` 等）与它们旁边的注释都留在原处。
    let renamed = request
        .original_id
        .as_deref()
        .filter(|original_id| *original_id != request.provider.id);
    if let Some(original_id) = renamed {
        remove_provider(&mut lines, original_id, layout)?;
    }

    match locate_provider(&lines, &request.provider.id, layout)? {
        Some((dictionary_indent, item)) => {
            let field_indent = dictionary_indent + 2;
            let mut updates = provider_field_updates(&request.provider, field_indent);
            // 文件里靠后的字段先套用：这样前面字段的行号移动不会影响它们。
            updates.reverse();
            apply_field_updates(&mut lines, &item, field_indent, updates)?;
        }
        None => insert_provider(&mut lines, &request.provider, layout)?,
    }

    let updated = join_lines(&lines);
    // 兜底：写入没有把顶层结构弄丢。这是唯一能在没有 YAML 解析器的情况下做的
    // 整体检查，它挡不住块内错误，但能挡住「整段被吃掉」这类致命失误。
    assert_top_level_kept(&before_lines, &lines, layout)?;

    write_text_atomic(&path, updated.as_bytes(), "dsh 设置文件")?;
    let backup = backup_path_of(&path);
    Ok(DshSettingsWriteResult {
        path: path.display().to_string(),
        revision: revision_of(&updated),
        backup_path: backup.display().to_string(),
    })
}

/// 把 `settings.yaml.imported` 里还没回到当前文档的供应商恢复回去。
/// 与单笔写入同一套纪律：revision 校验、顶层守卫、原子写、备份。恢复成功的
/// 供应商按残留文件顺序追加进当前文档；仍有问题的留在 `skipped` 里并说明原因。
#[tauri::command]
pub fn dsh_restore_imported_providers(
    request: DshRestoreImportedRequest,
) -> Result<DshRestoreImportedResult, String> {
    let (path, original, _exists, layout) = read_document()?;
    if revision_of(&original) != request.base_revision {
        return Err(
            "dsh 设置文件在读取之后被其它程序改过了，为避免覆盖对方的改动，本次恢复已取消。\
             请重新读取后再试。"
                .to_string(),
        );
    }
    let imported_path = dsh_imported_settings_path()?;
    let imported_text = std::fs::read_to_string(&imported_path).map_err(|error| {
        format!(
            "无法读取导入残留文件 {}：{error}",
            imported_path.display()
        )
    })?;
    let (updated, restored, skipped) =
        restore_imported_from_text(&original, &imported_text, layout)?;
    if restored.is_empty() && skipped.is_empty() {
        return Err("导入残留文件里没有可恢复的供应商（没有当前文档缺失的供应商）。".to_string());
    }
    if restored.is_empty() {
        // 一个都恢复不了时不落盘，revision 维持原值。
        return Ok(DshRestoreImportedResult {
            path: path.display().to_string(),
            revision: request.base_revision,
            backup_path: String::new(),
            restored,
            skipped,
        });
    }
    write_text_atomic(&path, updated.as_bytes(), "dsh 设置文件")?;
    Ok(DshRestoreImportedResult {
        path: path.display().to_string(),
        revision: revision_of(&updated),
        backup_path: backup_path_of(&path).display().to_string(),
        restored,
        skipped,
    })
}

/// 删除一个供应方。只移除它自己的块。
#[tauri::command]
pub fn dsh_delete_provider(
    request: DshDeleteProviderRequest,
) -> Result<DshSettingsWriteResult, String> {
    let (path, original, _, layout) = read_document()?;
    if revision_of(&original) != request.base_revision {
        return Err(
            "dsh 设置文件在读取之后被其它程序改过了，为避免覆盖对方的改动，本次删除已取消。\
             请重新读取后再试。"
                .to_string(),
        );
    }
    let mut lines = text_lines(&original);
    let before_lines = lines.clone();
    if !remove_provider(&mut lines, &request.provider_id, layout)? {
        return Err(format!(
            "设置文件里没有供应方 `{}`，可能已被其它程序删除，请重新读取。",
            request.provider_id
        ));
    }
    let updated = join_lines(&lines);
    assert_top_level_kept(&before_lines, &lines, layout)?;
    write_text_atomic(&path, updated.as_bytes(), "dsh 设置文件")?;
    Ok(DshSettingsWriteResult {
        path: path.display().to_string(),
        revision: revision_of(&updated),
        backup_path: backup_path_of(&path).display().to_string(),
    })
}

// ---------------------------------------------------------------------------
// 认证令牌（`$DSH_HOME/.credentials.yaml` 的 refs 分节）
//
// dsh 的密钥按名存放：settings.yaml 里只有引用名（`apiKeyEnv`），值在这个文件的
// `refs` 分节下，dsh 每次请求按引用名现读现用（热重载，改完即生效）。文件格式
// （对齐 dsh-credentials-local 的 README 与本机 0.1.5-rc.1 的实现）：
//
//     version: 1
//
//     refs:
//       DEEPSEEK_API_KEY: sk-…
//
//     records:
//       <owner>/<id>:
//         kind: grant
//         payload: …
//
// 与 settings.yaml 同一套纪律：按行最小手术，只动 `refs` 里启动器负责的那一行，
// `records` 与其余内容逐字节保留；不认识的写法一律报错而不是猜。与 dsh 一致的
// 两条规则：空值等于没有密钥（保存空值 = 移除那一行），值可以是任意单行文本
// （多行的块标量读不了也不会写，遇到就报错让人手工处理）。
// ---------------------------------------------------------------------------

const DSH_CREDENTIALS_FILE_NAME: &str = ".credentials.yaml";
const CREDENTIALS_VERSION_KEY: &str = "version";
const CREDENTIALS_VERSION: &str = "1";
const CREDENTIALS_REFS_KEY: &str = "refs";
const CREDENTIALS_RECORDS_KEY: &str = "records";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCredentialReadRequest {
    pub ref_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCredentialSaveRequest {
    /// settings.yaml 里这一供应方的路由键（文件里的 id）。
    pub provider_id: String,
    /// 令牌明文；空白 = 移除已存的值。
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCredentialSaveResult {
    /// 令牌实际存入的凭据引用名。
    pub ref_name: String,
    /// settings.yaml 被补写 `apiKeyEnv` 之后的修订；没动 settings.yaml 时为 null。
    pub settings_revision: Option<String>,
}

fn dsh_credentials_path() -> Result<PathBuf, String> {
    Ok(dsh_home()?.join(DSH_CREDENTIALS_FILE_NAME))
}

/// dsh 自己的引用名派生规则（`dsh-client-ui-settings-models/lib/client.js` 的
/// `deriveKeyRef`）：路由键大写、**连续的**非字母数字折叠成一个 `_`、后缀
/// `_API_KEY`。逐字符对照上游实现，包括首尾的下划线行为。
fn derive_credential_ref(provider_id: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for ch in provider_id.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    format!("{out}_API_KEY")
}

/// 从凭据文件文本里读出全部 `refs` 条目。识别两种布局：
///
/// - `version: 1` 的现行布局：值在 `refs:` 分节下；
/// - 预发布扁平布局（没有 `version` 行，根就是一张引用名映射）：dsh 启动时会把它
///   就地升级，这里只为读取做识别——写入一律拒绝，免得两边对同一文件各有各的写法。
fn parse_credential_refs(text: &str) -> Result<Vec<(String, String)>, String> {
    let lines = text_lines(text);
    let top = field_items(&lines, 0, lines.len(), 0)?;
    if !top.iter().any(|item| item.key == CREDENTIALS_VERSION_KEY) {
        let mut refs = Vec::new();
        for item in &top {
            if item.key == CREDENTIALS_RECORDS_KEY {
                return Err(
                    "凭据文件没有 version 行却带着 records 分节，布局无法识别；\
                     请先启动一次 dsh 让它升级。"
                        .to_string(),
                );
            }
            if let Some(value) = item.value.as_text() {
                refs.push((item.key.clone(), value));
            }
        }
        return Ok(refs);
    }
    let version = top
        .iter()
        .find(|item| item.key == CREDENTIALS_VERSION_KEY)
        .and_then(|item| item.value.as_text())
        .unwrap_or_default();
    if version != CREDENTIALS_VERSION {
        return Err(format!(
            "凭据文件的 version 是 `{version}`，启动器只认识 `{CREDENTIALS_VERSION}`。"
        ));
    }
    let Some(block) = find_block(&lines, 0, lines.len(), 0, CREDENTIALS_REFS_KEY)? else {
        return Ok(Vec::new());
    };
    let items = field_items(&lines, block.content_start, block.end, block.child_indent)?;
    let mut refs: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        // `key:` 后面什么都没有按「没有密钥」处理，与 dsh 的空值规则一致。
        let Some(value) = item.value.as_text() else {
            continue;
        };
        if seen.contains(&item.key) {
            return Err(format!(
                "凭据文件的 refs 里 `{}` 出现了不止一次，请先手工去重。",
                item.key
            ));
        }
        seen.insert(item.key.clone());
        refs.push((item.key, value));
    }
    Ok(refs)
}

/// 把一个引用名的值写进凭据文件文本：替换、插入或移除（空值）。只动 `refs` 里
/// 那一行，`records` 与其余内容逐字节保留。返回 (新文本, 是否有改动)。
fn apply_credential_ref(
    text: &str,
    ref_name: &str,
    value: &str,
) -> Result<(String, bool), String> {
    let mut lines = text_lines(text);
    let before_keys = top_level_keys(&lines);
    if !before_keys
        .iter()
        .any(|key| key == CREDENTIALS_VERSION_KEY)
    {
        return Err(
            "凭据文件还是旧版布局（没有 version 行）。先启动一次 dsh，它会自动升级；\
             之后启动器就能接管令牌的保存了。"
                .to_string(),
        );
    }

    let mut changed = false;
    match find_block(&lines, 0, lines.len(), 0, CREDENTIALS_REFS_KEY)? {
        Some(block) => {
            let items = field_items(&lines, block.content_start, block.end, block.child_indent)?;
            let pad = " ".repeat(block.child_indent);
            match items.iter().find(|item| item.key == ref_name) {
                Some(item) => {
                    let current = item.value.as_text().unwrap_or_default();
                    if current != value {
                        let end = item.content_end(&lines);
                        if value.is_empty() {
                            lines.drain(item.start..end);
                        } else {
                            lines.splice(
                                item.start..end,
                                vec![format!("{pad}{ref_name}: {}", render_scalar(value))],
                            );
                        }
                        changed = true;
                    }
                }
                None if !value.is_empty() => {
                    let line = format!("{pad}{ref_name}: {}", render_scalar(value));
                    let insert_at = append_index(&lines, block.content_start, block.end);
                    lines.splice(insert_at..insert_at, vec![line]);
                    changed = true;
                }
                None => {}
            }
        }
        None if !value.is_empty() => {
            // 文件还没有 refs 分节：补一节到文件尾（顶层顺序 dsh 不要求）。
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("{CREDENTIALS_REFS_KEY}:"));
            lines.push(format!("  {ref_name}: {}", render_scalar(value)));
            changed = true;
        }
        None => {}
    }
    if !changed {
        return Ok((text.to_string(), false));
    }

    // 移除最后一条引用后，空的 `refs:` 会被 dsh 当成 null 而不是映射拒绝，
    // 所以连分节行一起收走。这算「有意移除」，不算丢失分区。
    let mut removed_empty_refs_section = false;
    if value.is_empty() {
        if let Some(block) = find_block(&lines, 0, lines.len(), 0, CREDENTIALS_REFS_KEY)? {
            let still_has = field_items(
                &lines,
                block.content_start,
                block.end,
                block.child_indent,
            )?
            .iter()
            .any(|item| item.value.as_text().is_some());
            if !still_has {
                lines.drain(block.content_start - 1..block.end);
                removed_empty_refs_section = true;
            }
        }
    }

    // 顶层分区守卫：version / records 这些键一个都不能少。
    let after_keys = top_level_keys(&lines);
    for key in &before_keys {
        if *key == CREDENTIALS_REFS_KEY && removed_empty_refs_section {
            continue;
        }
        if !after_keys.contains(key) {
            return Err(format!(
                "写入结果丢失了凭据文件的顶层分区 `{key}`，已中止（文件未被修改）。"
            ));
        }
    }
    Ok((join_lines(&lines), true))
}

/// 设置文件里某供应方当前的 `apiKeyEnv`（已去空白）。没有供应方块或这一行
/// 都返回 None——两种情况由调用方区分。
fn existing_credential_ref(
    text: &str,
    provider_id: &str,
    layout: DshSettingsLayout,
) -> Result<Option<String>, String> {
    let lines = text_lines(text);
    let Some((dict_indent, item)) = locate_provider(&lines, provider_id, layout)? else {
        return Ok(None);
    };
    let fields = field_items(&lines, item.start + 1, item.end, dict_indent + 2)?;
    Ok(fields
        .iter()
        .find(|field| field.key == "apiKeyEnv")
        .and_then(|field| field.value.as_text())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

/// 往设置文件的某供应方块里补一行 `apiKeyEnv`。已有这一行时不动
/// （返回 None），供应方不存在时也返回 None——调用方负责先把这两种情况分清楚。
fn patch_provider_credential_ref(
    text: &str,
    provider_id: &str,
    ref_name: &str,
    layout: DshSettingsLayout,
) -> Result<Option<(String, String)>, String> {
    let lines = text_lines(text);
    let Some((dict_indent, item)) = locate_provider(&lines, provider_id, layout)? else {
        return Ok(None);
    };
    let field_indent = dict_indent + 2;
    let fields = field_items(&lines, item.start + 1, item.end, field_indent)?;
    if fields.iter().any(|field| field.key == "apiKeyEnv") {
        return Ok(None);
    }
    let mut lines = lines;
    let line = format!("{}apiKeyEnv: {}", " ".repeat(field_indent), render_scalar(ref_name));
    apply_field_updates(
        &mut lines,
        &item,
        field_indent,
        vec![("apiKeyEnv".to_string(), Some(vec![line]))],
    )?;
    assert_top_level_kept(&text_lines(text), &lines, layout)?;
    let updated = join_lines(&lines);
    let revision = revision_of(&updated);
    Ok(Some((updated, revision)))
}

/// 读取凭据文件里某个引用名的值。没有就返回 null——调用方据此决定输入框里
/// 放什么；启动环境里有同名变量时 dsh 会优先用它，这一层启动器看不到。
#[tauri::command]
pub fn dsh_read_credential(request: DshCredentialReadRequest) -> Result<Option<String>, String> {
    if !is_credential_ref_name(&request.ref_name) {
        return Err(format!(
            "`{}` 不是合法的凭据引用名（POSIX 变量名）。",
            request.ref_name
        ));
    }
    let path = dsh_credentials_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "无法读取 dsh 凭据文件 {}：{error}",
                path.display()
            ))
        }
    };
    let refs = parse_credential_refs(&text)?;
    Ok(refs
        .into_iter()
        .find(|(key, _)| *key == request.ref_name)
        .map(|(_, value)| value))
}

/// 保存（或移除）一个供应方的认证令牌。
///
/// 值写入凭据文件的 `refs`，引用名沿用设置文件里已有的 `apiKeyEnv`；没有时
/// 按 dsh 的派生规则从路由键生成，并把这一行补写进设置文件（只补这一行，
/// 其余逐字节保留）。先写凭据文件再动设置文件：前者失败时后者原封不动。
#[tauri::command]
pub fn dsh_save_credential(
    request: DshCredentialSaveRequest,
) -> Result<DshCredentialSaveResult, String> {
    let value = request.value.trim().to_string();

    // 1. 供应方必须已在设置文件里：令牌的引用名跟着路由键走，
    //    先有路由才谈得上令牌。
    let (settings_path, settings_text, _, layout) = read_document()?;
    {
        let lines = text_lines(&settings_text);
        if locate_provider(&lines, &request.provider_id, layout)?.is_none() {
            return Err(format!(
                "设置文件里还没有供应方 `{}`；先写入供应商，再保存令牌。",
                request.provider_id
            ));
        }
    }
    let existing_ref = existing_credential_ref(&settings_text, &request.provider_id, layout)?;
    let ref_name = match existing_ref.as_deref() {
        Some(name) => {
            if !is_credential_ref_name(name) {
                return Err(format!(
                    "settings.yaml 里的 apiKeyEnv `{name}` 不是合法的凭据引用名\
                     （POSIX 变量名），请先手工修正它。"
                ));
            }
            name.to_string()
        }
        None => {
            let derived = derive_credential_ref(&request.provider_id);
            if !is_credential_ref_name(&derived) {
                return Err(format!(
                    "无法从路由键 `{}` 派生出合法的凭据引用名（POSIX 变量名不能以\
                     数字开头），请在 settings.yaml 里为它手写一行 apiKeyEnv。",
                    request.provider_id
                ));
            }
            derived
        }
    };

    // 2. 凭据文件。文件不存在且要存值时，从带版本的空文档起步。
    let credentials_path = dsh_credentials_path()?;
    let credentials_text = match std::fs::read_to_string(&credentials_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(format!(
                "无法读取 dsh 凭据文件 {}：{error}",
                credentials_path.display()
            ))
        }
    };
    let (updated_credentials, credentials_changed) = if credentials_text.trim().is_empty() {
        if value.is_empty() {
            (credentials_text, false)
        } else {
            (
                format!(
                    "{CREDENTIALS_VERSION_KEY}: {CREDENTIALS_VERSION}\n\n\
                     {CREDENTIALS_REFS_KEY}:\n  {ref_name}: {}\n",
                    render_scalar(&value)
                ),
                true,
            )
        }
    } else {
        apply_credential_ref(&credentials_text, &ref_name, &value)?
    };
    if credentials_changed {
        write_private_text_atomic(
            &credentials_path,
            updated_credentials.as_bytes(),
            "dsh 凭据文件",
        )?;
    }

    // 3. 设置文件缺引用名时补一行（沿用现有引用名时不动）。
    let settings_revision = if existing_ref.is_none() {
        match patch_provider_credential_ref(&settings_text, &request.provider_id, &ref_name, layout)? {
            Some((updated, revision)) => {
                write_text_atomic(&settings_path, updated.as_bytes(), "dsh 设置文件")?;
                Some(revision)
            }
            None => None,
        }
    } else {
        None
    };

    Ok(DshCredentialSaveResult {
        ref_name,
        settings_revision,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCredentialRenameRequest {
    /// 凭据文件里现有的引用名。
    pub old_ref: String,
    /// 要换成的引用名。
    pub new_ref: String,
}

/// 把凭据文件文本里一个引用名的键换成新名字，**值原样保留**。返回 (新文本, 是否
/// 发生了换名)：旧名下没有值、或文件没有 refs 分节时返回原文本与 false——这两种
/// 都意味着「没有可换的东西」，不算错误。
///
/// 与 `apply_credential_ref` 同一套纪律：按行最小手术、写完检查顶层分区一个不少。
fn rename_credential_ref_in_text(
    text: &str,
    old_ref: &str,
    new_ref: &str,
) -> Result<(String, bool), String> {
    let mut lines = text_lines(text);
    let before_keys = top_level_keys(&lines);
    let Some(block) = find_block(&lines, 0, lines.len(), 0, CREDENTIALS_REFS_KEY)? else {
        return Ok((text.to_string(), false));
    };
    let items = field_items(&lines, block.content_start, block.end, block.child_indent)?;
    if items.iter().any(|item| item.key == new_ref) {
        return Err(format!(
            "凭据文件的 refs 里已经有 `{new_ref}` 了；为避免覆盖它的值，本次换名已取消。\
             请先手工处理凭据文件。"
        ));
    }
    let Some(item) = items.iter().find(|item| item.key == old_ref) else {
        return Ok((text.to_string(), false));
    };
    let pad = " ".repeat(block.child_indent);
    let value = item.value.as_text().unwrap_or_default();
    let end = item.content_end(&lines);
    lines.splice(
        item.start..end,
        vec![format!("{pad}{new_ref}: {}", render_scalar(&value))],
    );

    let after_keys = top_level_keys(&lines);
    for key in &before_keys {
        if !after_keys.contains(key) {
            return Err(format!(
                "换名结果丢失了凭据文件的顶层分区 `{key}`，已中止（文件未被修改）。"
            ));
        }
    }
    Ok((join_lines(&lines), true))
}

/// 把凭据文件里一个引用名换成新名字，**值原样保留**。
///
/// 供「供应商改了路由键」使用（2026-09-18，任务：改 id 令牌消失）：settings.yaml
/// 侧的引用名是派生的（没写 apiKeyEnv 行）时，改名后 dsh 会按新键派生新引用名，
/// 旧名下的值就悬空了。这里把旧名那行的键换成新名，令牌跟着供应商走。
/// settings.yaml 完全不动——它的 apiKeyEnv 行（如果有）与 id 无关，删块重建时
/// 整行保留。
#[tauri::command]
pub fn dsh_rename_credential_ref(
    request: DshCredentialRenameRequest,
) -> Result<(), String> {
    if !is_credential_ref_name(&request.old_ref) || !is_credential_ref_name(&request.new_ref)
    {
        return Err(format!(
            "凭据引用名 `{}` / `{}` 不是合法的 POSIX 变量名。",
            request.old_ref, request.new_ref
        ));
    }
    if request.old_ref == request.new_ref {
        return Ok(());
    }
    let path = dsh_credentials_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // 凭据文件还不存在 = 旧名下没有值，没有可换的东西，不算错误。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "无法读取 dsh 凭据文件 {}：{error}",
                path.display()
            ))
        }
    };
    let (updated, changed) =
        rename_credential_ref_in_text(&text, &request.old_ref, &request.new_ref)?;
    if changed {
        write_private_text_atomic(&path, updated.as_bytes(), "dsh 凭据文件")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# 用户自己的注释
ui-theme:
  preference: dark
llm-pi-ai:
  providers:
    kimi-coding:
      apiKeyEnv: KIMI_CODING_API_KEY
    vllm:
      apiKeyEnv: VLLM_API_KEY
      api: openai-completions
      baseURL: https://example.test/v1
      compat:
        thinkingFormat: qwen
      models:
        - id: Qwen3.8-27B
          name: Qwen3.8-27B
          reasoningEfforts:
            low: low
            max: max
agent-default-model:
  provider: kimi-coding
  model: k3-256k
  reasoningEffort: max
";

    fn providers_of(text: &str) -> Vec<DshProviderProfile> {
        parse_providers(&text_lines(text), DshSettingsLayout::LegacyYaml).expect("parse")
    }

    #[test]
    fn parses_every_provider_in_file_order() {
        let providers = providers_of(SAMPLE);
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].id, "kimi-coding");
        assert_eq!(providers[1].id, "vllm");
        assert_eq!(providers[1].api.as_deref(), Some("openai-completions"));
        assert_eq!(
            providers[1].base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(providers[1].models.len(), 1);
        let model = &providers[1].models[0];
        assert_eq!(model.id, "Qwen3.8-27B");
        assert_eq!(
            model.reasoning_efforts,
            vec![
                DshReasoningLevel {
                    level: "low".into(),
                    wire: Some("low".into())
                },
                DshReasoningLevel {
                    level: "max".into(),
                    wire: Some("max".into())
                },
            ]
        );
    }

    #[test]
    fn records_fields_the_launcher_does_not_own() {
        let providers = providers_of(SAMPLE);
        assert_eq!(providers[1].unmanaged_fields, vec!["compat".to_string()]);
        assert!(providers[0].unmanaged_fields.is_empty());
    }

    #[test]
    fn survives_without_the_section() {
        let providers = providers_of("ui-theme:\n  preference: dark\n");
        assert!(providers.is_empty());
    }

    #[test]
    fn rejects_tabs_in_indentation() {
        let text = "llm-pi-ai:\n  providers:\n\tvllm:\n      apiKeyEnv: X\n";
        assert!(parse_providers(&text_lines(text), DshSettingsLayout::LegacyYaml).is_err());
    }

    #[test]
    fn rejects_block_scalars_for_managed_fields() {
        let text = "llm-pi-ai:\n  providers:\n    vllm:\n      baseURL: |\n        https://x\n";
        assert!(parse_providers(&text_lines(text), DshSettingsLayout::LegacyYaml).is_err());
    }

    /// 走真实的「读 → 改一个字段 → 写」链路，而不是在测试里复制一遍写入逻辑：
    /// 这条断言是「只有一个字段被重写、其余逐字节保留」这个承诺的唯一守卫。
    #[test]
    fn rewriting_one_field_keeps_everything_else_byte_identical() {
        let mut lines = text_lines(SAMPLE);
        let (dictionary_indent, item) = locate_provider(&lines, "vllm", DshSettingsLayout::LegacyYaml)
            .expect("locate")
            .expect("vllm 应当存在于样例里");
        assert_eq!(dictionary_indent, 4);

        // 模拟用户在界面上只改了 API 地址。
        let mut provider = providers_of(SAMPLE)
            .into_iter()
            .find(|provider| provider.id == "vllm")
            .expect("vllm");
        provider.base_url = Some("https://changed.test/v1".into());

        let mut updates = provider_field_updates(&provider, dictionary_indent + 2);
        updates.reverse();
        apply_field_updates(&mut lines, &item, dictionary_indent + 2, updates).expect("apply");
        let updated = join_lines(&lines);

        assert!(
            updated.starts_with("# 用户自己的注释\n"),
            "文件头的注释必须原样保留，实际：{updated}"
        );
        assert!(updated.contains("https://changed.test/v1"));
        // 启动器不负责的字段连同它们的缩进一起原样保留。
        assert!(updated.contains("compat:\n        thinkingFormat: qwen"));
        // 没动的模型子树原样保留。
        assert!(updated.contains("reasoningEfforts:\n            low: low\n            max: max"));
        // 其它顶层分区不受影响。
        assert!(updated.contains("agent-default-model:\n  provider: kimi-coding"));
        // 另一个供应商一个字节都没变。
        assert!(updated.contains("    kimi-coding:\n      apiKeyEnv: KIMI_CODING_API_KEY\n"));
        assert_eq!(
            updated.lines().count(),
            SAMPLE.lines().count(),
            "只改一个标量不该增减行数"
        );
    }

    /// 同一个 `providers` 字典下，每个供应商的字段缩进都一样，所以「本供应商缺
    /// 某个字段」绝不能因为查找范围越界而落到下一个供应商头上——那会静默改坏别人
    /// 的配置。触发条件是块内先发生收缩（models 变短），随后的字段查找就会伸出去，
    /// 而且**伸出的行数必须够到下一个供应商的同名/相邻字段**才看得出来；这里的
    /// aaa 从三条模型收成一条（收缩 4 行），正好够到 bbb 的 `reasoning`。
    #[test]
    fn a_shrinking_block_never_reaches_into_the_next_provider() {
        let text = "llm-pi-ai:\n  providers:\n    \
            aaa:\n      apiKeyEnv: A\n      models:\n\
            \x20       - id: one\n          name: one\n\
            \x20       - id: two\n          name: two\n\
            \x20       - id: three\n          name: three\n    \
            bbb:\n      apiKeyEnv: B\n      reasoning: high\n";
        let mut lines = text_lines(text);
        let (dictionary_indent, item) = locate_provider(&lines, "aaa", DshSettingsLayout::LegacyYaml)
            .expect("locate")
            .expect("aaa");

        let mut provider = providers_of(text)
            .into_iter()
            .find(|provider| provider.id == "aaa")
            .expect("aaa");
        // aaa 自己没有 reasoning，bbb 有——这正是会张冠李戴的那一对。
        assert_eq!(provider.reasoning, None);
        assert_eq!(provider.models.len(), 3, "夹具应有三条模型，收缩才够大");
        // aaa 的模型从三条收成一条，块因此收缩 4 行。
        provider.models.truncate(1);

        let mut updates = provider_field_updates(&provider, dictionary_indent + 2);
        updates.reverse();
        apply_field_updates(&mut lines, &item, dictionary_indent + 2, updates).expect("apply");
        let updated = join_lines(&lines);

        assert!(
            updated.contains("    bbb:\n      apiKeyEnv: B\n      reasoning: high\n"),
            "下一个供应商必须逐字节不变，实际：{updated}"
        );
        assert_eq!(updated.matches("id: two").count(), 0, "被删掉的模型应当消失");
        assert_eq!(updated.matches("id: three").count(), 0, "被删掉的模型应当消失");
        assert_eq!(updated.matches("id: one").count(), 1);
    }

    #[test]
    fn inserting_a_provider_appends_inside_the_dict() {
        let mut lines = text_lines("llm-pi-ai:\n  providers:\n    a:\n      apiKeyEnv: A\n");
        insert_provider(
            &mut lines,
            &DshProviderProfile {
                id: "b".into(),
                api_key_env: Some("B".into()),
                ..Default::default()
            },
            DshSettingsLayout::LegacyYaml,
        )
        .expect("insert");
        let updated = join_lines(&lines);
        assert_eq!(
            updated,
            "llm-pi-ai:\n  providers:\n    a:\n      apiKeyEnv: A\n    b:\n      apiKeyEnv: B\n"
        );
    }

    #[test]
    fn inserting_creates_missing_sections() {
        let mut lines = text_lines("ui-theme:\n  preference: dark\n");
        insert_provider(
            &mut lines,
            &DshProviderProfile {
                id: "vllm".into(),
                api_key_env: Some("VLLM_API_KEY".into()),
                ..Default::default()
            },
            DshSettingsLayout::LegacyYaml,
        )
        .expect("insert");
        let updated = join_lines(&lines);
        assert!(updated.starts_with("llm-pi-ai:\n  providers:\n    vllm:\n"));
        assert!(updated.contains("ui-theme:\n  preference: dark"));
    }

    #[test]
    fn removing_a_provider_takes_its_leading_comment() {
        let text = "llm-pi-ai:\n  providers:\n    # 老网关\n    old:\n      apiKeyEnv: OLD\n    keep:\n      apiKeyEnv: KEEP\n";
        let mut lines = text_lines(text);
        assert!(remove_provider(&mut lines, "old", DshSettingsLayout::LegacyYaml).expect("remove"));
        assert_eq!(
            join_lines(&lines),
            "llm-pi-ai:\n  providers:\n    keep:\n      apiKeyEnv: KEEP\n"
        );
    }

    #[test]
    fn renders_models_with_reasoning_levels() {
        let model = DshModelProfile {
            id: "m".into(),
            name: Some("M".into()),
            context_window: Some(262144),
            reasoning_efforts: vec![
                DshReasoningLevel {
                    level: "off".into(),
                    wire: None,
                },
                DshReasoningLevel {
                    level: "max".into(),
                    wire: Some("none".into()),
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            render_model(&model, 4),
            vec![
                "    - id: m",
                "      name: M",
                "      contextWindow: 262144",
                "      reasoningEfforts:",
                // 只有 off 允许留空：表示「支持这一档但不发送参数」。
                "        off:",
                "        max: none",
            ]
        );
    }

    #[test]
    fn quotes_only_the_scalars_yaml_would_misread() {
        // vLLM 的 `none` 是普普通通的字符串，加引号反而画蛇添足。
        assert_eq!(render_scalar("none"), "none");
        assert_eq!(render_scalar("openai-completions"), "openai-completions");
        assert_eq!(render_scalar("https://example.test/v1"), "https://example.test/v1");
        // `off` 是 YAML 1.1 的布尔字面量，必须引起来才不会被读成 false。
        assert_eq!(render_scalar("off"), "'off'");
        assert_eq!(render_scalar("true"), "'true'");
        // 纯数字会被读成整数。
        assert_eq!(render_scalar("123"), "'123'");
        // 冒号加空格是映射分隔符，URL 里的冒号则不是——前者必须引。
        assert_eq!(render_scalar("a: b"), "'a: b'");
        assert_eq!(render_scalar(""), "''");
        // 空白后的 # 会被读成注释起始，值会被截断。
        assert_eq!(render_scalar("value # 注释"), "'value # 注释'");
        // 中间的撇号是合法的普通标量，不需要引号。
        assert_eq!(render_scalar("it's"), "it's");
    }

    #[test]
    fn validate_mirrors_the_dsh_rules() {
        let mut model = DshModelProfile {
            id: "m".into(),
            reasoning_efforts: vec![DshReasoningLevel {
                level: "max".into(),
                wire: None,
            }],
            ..Default::default()
        };
        assert!(validate_reasoning_efforts(&model).is_err());

        model.reasoning_efforts = vec![DshReasoningLevel {
            level: "off".into(),
            wire: None,
        }];
        assert!(validate_reasoning_efforts(&model).is_err(), "只有 off 也应被拒");

        model.reasoning_efforts = vec![DshReasoningLevel {
            level: "turbo".into(),
            wire: Some("turbo".into()),
        }];
        assert!(validate_reasoning_efforts(&model).is_err());

        model.reasoning_efforts = vec![DshReasoningLevel {
            level: "off".into(),
            wire: None,
        }];
        assert!(validate_reasoning_efforts(&model).is_err());

        assert!(validate_provider(&DshProviderProfile {
            id: "Bad_Id".into(),
            ..Default::default()
        })
        .is_err());

        // 凭据引用名照 dsh 的 `credentialRef()` 规则：POSIX shell 变量名。
        let with_ref = |reference: Option<&str>| DshProviderProfile {
            id: "my-gateway".into(),
            api_key_env: reference.map(str::to_string),
            ..Default::default()
        };
        // 合法：字母/下划线开头、只含字母数字下划线；留空与缺省也合法。
        assert!(validate_provider(&with_ref(Some("MY_GATEWAY_API_KEY"))).is_ok());
        assert!(validate_provider(&with_ref(Some("_PRIVATE"))).is_ok());
        assert!(validate_provider(&with_ref(Some("K1"))).is_ok());
        assert!(validate_provider(&with_ref(Some(""))).is_ok());
        assert!(validate_provider(&with_ref(None)).is_ok());
        // 不合法：连字符（最容易踩的一处——路由键里有连字符不代表这里能有）、
        // 点、空格、数字开头。
        for bad in ["MY-GATEWAY_API_KEY", "MY.GATEWAY_KEY", "MY GATEWAY KEY", "1KEY", " "] {
            assert!(
                validate_provider(&with_ref(Some(bad))).is_err(),
                "凭据引用名 `{bad}` 应被拒"
            );
        }
    }

    #[test]
    fn top_level_keys_detects_a_lost_section() {
        let before = top_level_keys(&text_lines(SAMPLE));
        assert_eq!(before, vec!["ui-theme", "llm-pi-ai", "agent-default-model"]);
    }

    /// 「自定义」的判据与 dsh 自己的设置页同源：路由键在不在 dsh 自带的目录里。
    /// SAMPLE 里那两条正是真实文件里的形状——`kimi-coding` 只有一把密钥（目录路由），
    /// `vllm` 声明了协议与端点（自定义）。
    #[test]
    fn custom_is_decided_by_the_installed_catalog() {
        let catalog: std::collections::HashSet<String> =
            ["kimi-coding", "openai"].iter().map(|id| id.to_string()).collect();
        assert!(!is_custom_route("kimi-coding", Some(&catalog)));
        assert!(is_custom_route("vllm", Some(&catalog)));
        // 目录清单读不到时按自定义处理：宁可多列，也不把用户的路由藏起来。
        assert!(is_custom_route("kimi-coding", None));

        let mut providers = providers_of(SAMPLE);
        let catalog = Some(catalog);
        for provider in &mut providers {
            provider.custom = is_custom_route(&provider.id, catalog.as_ref());
        }
        assert_eq!(
            providers
                .iter()
                .map(|provider| (provider.id.as_str(), provider.custom))
                .collect::<Vec<_>>(),
            vec![("kimi-coding", false), ("vllm", true)]
        );
    }

    /// `custom` 是算出来的、不是文件里的字段：写回时不能被渲染进 YAML。
    #[test]
    fn custom_never_reaches_the_file() {
        let provider = DshProviderProfile {
            id: "vllm".into(),
            api: Some("openai-completions".into()),
            custom: true,
            ..Default::default()
        };
        let rendered = render_provider(&provider, 4).join("\n");
        assert!(!rendered.contains("custom"));
        assert!(!MANAGED_PROVIDER_KEYS.contains(&"custom"));
    }

    /// 目录清单的**路径**也得在真机上对得上：读不到不会报错，只是过滤不再生效
    /// （界面退化成把目录路由也列出来），所以这里主动探一次。机器上还没跑过 dsh
    /// （profile 未安装）时跳过——那是正常状态，不是失败。
    #[test]
    fn the_installed_catalog_is_reachable_when_present() {
        let Some(routes) = installed_catalog_routes(DshSettingsLayout::LegacyYaml) else {
            return;
        };
        // 挑两个确认读到的确实是 pi-ai 的目录：一个是它自带的产品路由，一个是
        // 真实文件里出现过的目录路由。
        assert!(routes.contains("openai"), "读到的目录清单不像 pi-ai 的");
        assert!(routes.contains("kimi-coding"));
        assert!(!routes.contains("vllm"), "自定义路由不该出现在目录清单里");
    }

    // -- 认证令牌（凭据文件的 refs 分节） -----------------------------------

    const CREDENTIALS_SAMPLE: &str = "\
version: 1

# dsh 自己的记录
records:
  client-connection/browser-session:
    kind: grant
    payload:
      version: 1
      secret: xxx

refs:
  DEEPSEEK_API_KEY: sk-old
  KIMI_CODING_API_KEY: 'sk-quoted'
";

    #[test]
    fn reads_refs_from_a_versioned_credentials_file() {
        let refs = parse_credential_refs(CREDENTIALS_SAMPLE).expect("parse");
        assert_eq!(
            refs,
            vec![
                ("DEEPSEEK_API_KEY".to_string(), "sk-old".to_string()),
                ("KIMI_CODING_API_KEY".to_string(), "sk-quoted".to_string()),
            ]
        );
    }

    #[test]
    fn reads_the_prelaunch_flat_layout_but_refuses_to_write_it() {
        let flat = "DEEPSEEK_API_KEY: sk-old\nOTHER_KEY: v2\n";
        let refs = parse_credential_refs(flat).expect("parse flat");
        assert_eq!(refs.len(), 2);

        let applied = apply_credential_ref(flat, "DEEPSEEK_API_KEY", "sk-new");
        assert!(applied.is_err(), "旧版布局不许被启动器改写");
    }

    #[test]
    fn replacing_one_ref_keeps_everything_else_byte_identical() {
        let (updated, changed) =
            apply_credential_ref(CREDENTIALS_SAMPLE, "DEEPSEEK_API_KEY", "sk-new")
                .expect("apply");
        assert!(changed);
        // records 分节与其注释逐字节保留。
        assert!(updated.contains("# dsh 自己的记录"));
        assert!(updated.contains("client-connection/browser-session:"));
        assert!(updated.contains("      secret: xxx"));
        assert!(updated.contains("KIMI_CODING_API_KEY: 'sk-quoted'"));
        assert!(updated.contains("DEEPSEEK_API_KEY: sk-new"));
        // version 行还在最前面。
        assert!(updated.starts_with("version: 1"));
    }

    #[test]
    fn saving_the_same_value_is_a_noop() {
        let (updated, changed) =
            apply_credential_ref(CREDENTIALS_SAMPLE, "DEEPSEEK_API_KEY", "sk-old")
                .expect("apply");
        assert!(!changed);
        assert_eq!(updated, CREDENTIALS_SAMPLE);
    }

    #[test]
    fn a_new_ref_appends_inside_the_refs_block() {
        let (updated, changed) =
            apply_credential_ref(CREDENTIALS_SAMPLE, "VLLM_API_KEY", "vllm-token")
                .expect("apply");
        assert!(changed);
        let refs_at = updated.find("refs:").expect("refs block");
        let new_at = updated.find("VLLM_API_KEY: vllm-token").expect("new ref");
        assert!(new_at > refs_at);
        // 追加落在 refs 分节内，不会跑到文件尾的空白行之后去。
        assert!(!updated.contains("VLLM_API_KEY: vllm-token\n\n"));
    }

    #[test]
    fn empty_value_removes_the_ref_and_an_empty_section_goes_too() {
        let (updated, changed) =
            apply_credential_ref(CREDENTIALS_SAMPLE, "DEEPSEEK_API_KEY", "").expect("apply");
        assert!(changed);
        assert!(!updated.contains("DEEPSEEK_API_KEY"));
        assert!(updated.contains("refs:"), "还有别的引用，分节要留着");

        // 只剩一条时移除它，空的 refs: 连分节行一起收走（dsh 把 null 拒之门外）。
        let single = "version: 1\n\nrefs:\n  ONLY_KEY: v\n";
        let (updated, changed) = apply_credential_ref(single, "ONLY_KEY", "").expect("apply");
        assert!(changed);
        assert!(!updated.contains("refs:"));
        assert!(updated.starts_with("version: 1"));
    }

    #[test]
    fn a_missing_refs_section_is_created() {
        let bare = "version: 1\n\nrecords:\n  a/b:\n    kind: grant\n";
        let (updated, changed) = apply_credential_ref(bare, "NEW_KEY", "v").expect("apply");
        assert!(changed);
        assert!(updated.contains("refs:\n  NEW_KEY: v"));
        assert!(updated.contains("records:"));
    }

    #[test]
    fn saving_into_a_missing_file_starts_a_versioned_document() {
        // 这个路径在 dsh_save_credential 里：空文本 + 非空值 → 直接生成带版本的
        // 起步文档。这里验证它过得了 apply 的解析（apply 只认 versioned 布局）。
        let fresh = format!(
            "version: 1\n\nrefs:\n  {}: v\n",
            derive_credential_ref("my-gateway")
        );
        let refs = parse_credential_refs(&fresh).expect("parse");
        assert_eq!(refs, vec![("MY_GATEWAY_API_KEY".to_string(), "v".to_string())]);
    }

    #[test]
    fn derived_ref_names_match_dsh() {
        assert_eq!(derive_credential_ref("vllm"), "VLLM_API_KEY");
        assert_eq!(derive_credential_ref("my-gateway"), "MY_GATEWAY_API_KEY");
        // 连续的非字母数字折叠成一个下划线（上游 replace(/[^A-Z0-9]+/g, "_")）。
        assert_eq!(derive_credential_ref("my--gw"), "MY_GW_API_KEY");
        assert_eq!(derive_credential_ref("openai-codex"), "OPENAI_CODEX_API_KEY");
    }

    #[test]
    fn renaming_a_credential_ref_keeps_the_value_and_the_rest() {
        let text = "version: 1\n\nrefs:\n  VLLM_API_KEY: sk-secret\n  OTHER_KEY: keep-me\n\n\
                    records:\n  a/b:\n    kind: grant\n";
        let (updated, changed) =
            rename_credential_ref_in_text(text, "VLLM_API_KEY", "VLLM2_API_KEY")
                .expect("rename");
        assert!(changed);
        // 值原样跟着新名走，其它条目与 records 分节逐字节保留。
        assert!(updated.contains("VLLM2_API_KEY: sk-secret"));
        assert!(!updated.contains("VLLM_API_KEY: sk-secret"));
        assert!(updated.contains("OTHER_KEY: keep-me"));
        assert!(updated.contains("records:\n  a/b:\n    kind: grant"));

        // 旧名下没有值 = 无事发生，不算错误。
        let (same, changed) =
            rename_credential_ref_in_text(text, "NOPE_API_KEY", "X_API_KEY").expect("rename");
        assert!(!changed);
        assert_eq!(same, text);

        // 新名已被占用时拒绝，免得覆盖别人的值。
        assert!(
            rename_credential_ref_in_text(text, "VLLM_API_KEY", "OTHER_KEY").is_err()
        );
    }

    #[test]
    fn the_api_key_env_line_is_added_only_when_missing() {
        let settings = "llm-pi-ai:\n  providers:\n    vllm:\n      api: openai-completions\n";
        let (updated, revision) =
            patch_provider_credential_ref(settings, "vllm", "VLLM_API_KEY", DshSettingsLayout::LegacyYaml)
                .expect("patch")
                .expect("patched");
        assert!(updated.contains("apiKeyEnv: VLLM_API_KEY"));
        assert!(updated.contains("api: openai-completions"));
        assert!(!revision.is_empty());

        // 已有这一行（哪怕值不同）就不动——令牌会存进那个已有引用名下。
        let with_ref = "llm-pi-ai:\n  providers:\n    vllm:\n      apiKeyEnv: CUSTOM_KEY\n";
        assert!(
            patch_provider_credential_ref(with_ref, "vllm", "VLLM_API_KEY", DshSettingsLayout::LegacyYaml)
                .expect("patch")
                .is_none()
        );
        // 供应方不存在时同样不动。
        assert!(
            patch_provider_credential_ref(settings, "nope", "X_API_KEY", DshSettingsLayout::LegacyYaml)
                .expect("patch")
                .is_none()
        );
    }

    // -- profile patch 布局（dsh 0.2.0 起） ---------------------------------

    /// 形状取真机的 `profiles/web/cordis.patch.yml`：dsh 设置页写出的若干 entry、
    /// 文件头注释、以及一个带 `!!js` 表达式的 entry（dsh 的合法写法，启动器只定位
    /// 边界、不解析内容）。
    const PATCH_SAMPLE: &str = "\
# Your patch layer for this dsh profile, applied after every bundle layer:
# a top-level YAML array of loader patch entries (id-targeted config
# overrides, disables, and insert lists; `!!js` expressions allowed).
- id: ui-theme
  name: \"@deepseek-ai/dsh-client-ui-theme\"
  config:
    preference: dark
- id: webserver
  config:
    port: !!js ctx.webStartup.port ?? 3080
- id: llm-pi-ai
  name: \"@deepseek-ai/dsh-llm-pi-ai\"
  config:
    providers:
      kimi-coding:
        apiKeyEnv: KIMI_CODING_API_KEY
      vllm:
        api: openai-completions
        baseURL: https://example.test/v1
        compat:
          thinkingFormat: qwen
        models:
          - id: Qwen3.8-27B
            reasoningEfforts:
              max: max
- id: agent-default-model
  name: \"@deepseek-ai/dsh-agent-default-model\"
  config:
    provider: kimi-coding
    model: k3-256k
";

    fn patch_providers_of(text: &str) -> Vec<DshProviderProfile> {
        parse_providers(&text_lines(text), DshSettingsLayout::ProfilePatch).expect("parse")
    }

    #[test]
    fn patch_layout_parses_providers_from_the_entry_config() {
        let providers = patch_providers_of(PATCH_SAMPLE);
        assert_eq!(
            providers.iter().map(|provider| provider.id.as_str()).collect::<Vec<_>>(),
            vec!["kimi-coding", "vllm"]
        );
        assert_eq!(providers[1].api.as_deref(), Some("openai-completions"));
        assert_eq!(providers[1].unmanaged_fields, vec!["compat".to_string()]);
        assert_eq!(providers[1].models[0].id, "Qwen3.8-27B");
    }

    #[test]
    fn patch_layout_survives_js_expressions_in_other_entries() {
        // `!!js` 在 webserver entry 里：解析 llm-pi-ai 时绝不能因为它报错。
        assert_eq!(patch_providers_of(PATCH_SAMPLE).len(), 2);
        // 顶层出现非序列内容才是真的坏文件，必须 fail loud。
        let broken = "ui-theme:\n  preference: dark\n";
        assert!(parse_providers(&text_lines(broken), DshSettingsLayout::ProfilePatch).is_err());
    }

    #[test]
    fn patch_layout_empty_document_and_placeholder_have_no_providers() {
        assert!(patch_providers_of("").is_empty());
        // dsh 初始化 profile 时的空序列占位。
        let placeholder = "# header comment\n[]\n";
        assert!(patch_providers_of(placeholder).is_empty());
    }

    #[test]
    fn patch_layout_insert_into_existing_providers_keeps_everything_else() {
        let mut lines = text_lines(PATCH_SAMPLE);
        let provider = DshProviderProfile {
            id: "strata".into(),
            api: Some("openai-completions".into()),
            base_url: Some("http://100.106.72.126:8080/v1".into()),
            reasoning: Some("max".into()),
            models: vec![DshModelProfile {
                id: "qwen3.8-flash-next-iq3_s".into(),
                context_window: Some(262144),
                ..Default::default()
            }],
            ..Default::default()
        };
        insert_provider(&mut lines, &provider, DshSettingsLayout::ProfilePatch).expect("insert");
        let updated = join_lines(&lines);

        // 新供应方落在 llm-pi-ai entry 的 providers 字典里、缩进与既有条目一致。
        assert!(
            updated.contains(
                "      vllm:\n        api: openai-completions\n        baseURL: https://example.test/v1\n        compat:\n          thinkingFormat: qwen\n        models:\n          - id: Qwen3.8-27B\n            reasoningEfforts:\n              max: max\n      strata:\n        api: openai-completions\n        baseURL: http://100.106.72.126:8080/v1\n        reasoning: max\n        models:\n          - id: qwen3.8-flash-next-iq3_s\n            contextWindow: 262144\n"
            ),
            "实际：{updated}"
        );
        // 其它 entry（含 `!!js` 那条）与文件头注释逐字节保留。
        assert!(updated.starts_with("# Your patch layer for this dsh profile"));
        assert!(updated.contains("- id: webserver\n  config:\n    port: !!js ctx.webStartup.port ?? 3080\n"));
        assert!(updated.contains("- id: agent-default-model\n  name: \"@deepseek-ai/dsh-agent-default-model\"\n  config:\n    provider: kimi-coding\n    model: k3-256k\n"));
        // 守卫：entry 一个没少。
        assert_top_level_kept(&text_lines(PATCH_SAMPLE), &lines, DshSettingsLayout::ProfilePatch)
            .expect("guard");
    }

    #[test]
    fn patch_layout_insert_creates_the_entry_when_missing() {
        // 没有 llm-pi-ai entry：追加完整 entry 到文件尾，形状照 dsh configEditor 写的。
        let text = "# header\n- id: ui-theme\n  config:\n    preference: dark\n";
        let mut lines = text_lines(text);
        let provider = DshProviderProfile {
            id: "vllm".into(),
            api_key_env: Some("VLLM_API_KEY".into()),
            ..Default::default()
        };
        insert_provider(&mut lines, &provider, DshSettingsLayout::ProfilePatch).expect("insert");
        let updated = join_lines(&lines);
        assert!(updated.starts_with("# header\n- id: ui-theme\n  config:\n    preference: dark\n"));
        assert!(
            updated.contains(
                "- id: llm-pi-ai\n  name: '@deepseek-ai/dsh-llm-pi-ai'\n  config:\n    providers:\n      vllm:\n        apiKeyEnv: VLLM_API_KEY\n"
            ),
            "实际：{updated}"
        );
        // 再读一次：新 entry 里的供应方能被解析回来。
        let parsed = patch_providers_of(&updated);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "vllm");
    }

    #[test]
    fn patch_layout_insert_replaces_the_empty_sequence_placeholder() {
        let text = "# header\n[]\n";
        let mut lines = text_lines(text);
        let provider = DshProviderProfile {
            id: "vllm".into(),
            api: Some("openai-completions".into()),
            ..Default::default()
        };
        insert_provider(&mut lines, &provider, DshSettingsLayout::ProfilePatch).expect("insert");
        let updated = join_lines(&lines);
        assert!(!updated.contains("[]"), "占位行必须被收走，实际：{updated}");
        assert!(updated.starts_with("# header\n- id: llm-pi-ai\n"));
    }

    #[test]
    fn patch_layout_insert_fills_a_configless_entry() {
        // entry 在但只带过别的字段（不太可能，但要能补）：补 config 与 providers。
        let text = "- id: llm-pi-ai\n  name: \"@deepseek-ai/dsh-llm-pi-ai\"\n";
        let mut lines = text_lines(text);
        let provider = DshProviderProfile {
            id: "vllm".into(),
            api_key_env: Some("VLLM_API_KEY".into()),
            ..Default::default()
        };
        insert_provider(&mut lines, &provider, DshSettingsLayout::ProfilePatch).expect("insert");
        let updated = join_lines(&lines);
        assert_eq!(
            updated,
            "- id: llm-pi-ai\n  name: \"@deepseek-ai/dsh-llm-pi-ai\"\n  config:\n    providers:\n      vllm:\n        apiKeyEnv: VLLM_API_KEY\n"
        );
    }

    #[test]
    fn patch_layout_insert_fills_providers_under_existing_config() {
        let text = "- id: llm-pi-ai\n  config:\n    modelOverrides: {}\n";
        let mut lines = text_lines(text);
        let provider = DshProviderProfile {
            id: "vllm".into(),
            api_key_env: Some("VLLM_API_KEY".into()),
            ..Default::default()
        };
        insert_provider(&mut lines, &provider, DshSettingsLayout::ProfilePatch).expect("insert");
        let updated = join_lines(&lines);
        assert!(updated.contains("    modelOverrides: {}\n    providers:\n      vllm:\n        apiKeyEnv: VLLM_API_KEY\n"), "实际：{updated}");
    }

    #[test]
    fn patch_layout_uses_the_last_non_insert_entry() {
        // 两条同名 entry：一条带 insert（插入清单，不是 config 归属行），一条是
        // 正常的 config 行。定位必须与 dsh 的 findLastIndex 一致——落在后者。
        let text = "\
- id: llm-pi-ai
  insert: before
  config:
    providers:
      decoy:
        apiKeyEnv: DECOY
- id: llm-pi-ai
  name: \"@deepseek-ai/dsh-llm-pi-ai\"
  config:
    providers:
      real:
        apiKeyEnv: REAL
";
        let providers = patch_providers_of(text);
        assert_eq!(
            providers.iter().map(|provider| provider.id.as_str()).collect::<Vec<_>>(),
            vec!["real"]
        );
    }

    #[test]
    fn patch_layout_edit_keeps_other_entries_byte_identical() {
        let mut lines = text_lines(PATCH_SAMPLE);
        let (dictionary_indent, item) =
            locate_provider(&lines, "vllm", DshSettingsLayout::ProfilePatch)
                .expect("locate")
                .expect("vllm");
        assert_eq!(dictionary_indent, 6);

        let mut provider = patch_providers_of(PATCH_SAMPLE)
            .into_iter()
            .find(|provider| provider.id == "vllm")
            .expect("vllm");
        provider.base_url = Some("https://changed.test/v1".into());

        let mut updates = provider_field_updates(&provider, dictionary_indent + 2);
        updates.reverse();
        apply_field_updates(&mut lines, &item, dictionary_indent + 2, updates).expect("apply");
        let updated = join_lines(&lines);

        assert!(updated.contains("baseURL: https://changed.test/v1"));
        // 没动的供应商与其它 entry 一字节不变。
        assert!(updated.contains("      kimi-coding:\n        apiKeyEnv: KIMI_CODING_API_KEY\n"));
        assert!(updated.contains("        compat:\n          thinkingFormat: qwen"));
        assert!(updated.contains("- id: webserver\n  config:\n    port: !!js ctx.webStartup.port ?? 3080\n"));
        assert_eq!(updated.lines().count(), PATCH_SAMPLE.lines().count());
    }

    #[test]
    fn patch_layout_remove_takes_only_its_own_block() {
        let mut lines = text_lines(PATCH_SAMPLE);
        assert!(remove_provider(&mut lines, "vllm", DshSettingsLayout::ProfilePatch).expect("remove"));
        let updated = join_lines(&lines);
        assert!(!updated.contains("vllm"));
        assert!(updated.contains("      kimi-coding:\n        apiKeyEnv: KIMI_CODING_API_KEY\n"));
        assert!(updated.contains("- id: agent-default-model\n"));
        assert_top_level_kept(&text_lines(PATCH_SAMPLE), &lines, DshSettingsLayout::ProfilePatch)
            .expect("guard");
    }

    #[test]
    fn patch_layout_guard_catches_a_lost_entry() {
        let before = text_lines(PATCH_SAMPLE);
        // 模拟一次把 agent-default-model entry 整个吃掉的错误写入。
        let cut = before
            .iter()
            .position(|line| line == "- id: agent-default-model")
            .expect("entry line");
        let after: Vec<String> = before[..cut].to_vec();
        assert!(assert_top_level_kept(&before, &after, DshSettingsLayout::ProfilePatch).is_err());
    }

    #[test]
    fn patch_layout_credential_ref_patch_works_inside_the_entry() {
        let (updated, _) = patch_provider_credential_ref(
            PATCH_SAMPLE,
            "vllm",
            "VLLM_API_KEY",
            DshSettingsLayout::ProfilePatch,
        )
        .expect("patch")
        .expect("patched");
        // 新字段追加在供应方块的尾部（与旧布局同一套 append 规则），块内其余不变。
        assert!(updated.contains("        apiKeyEnv: VLLM_API_KEY\n"), "实际：{updated}");
        assert!(updated.contains("      vllm:\n        api: openai-completions\n"));
        assert!(updated.contains("            reasoningEfforts:\n              max: max\n"));
        // 已有 apiKeyEnv 的不动。
        assert!(
            patch_provider_credential_ref(PATCH_SAMPLE, "kimi-coding", "X", DshSettingsLayout::ProfilePatch)
                .expect("patch")
                .is_none()
        );
    }

    #[test]
    fn layout_detection_parses_versions() {
        assert_eq!(layout_for_version("0.1.5-rc.1"), Some(DshSettingsLayout::LegacyYaml));
        assert_eq!(layout_for_version("0.1.9"), Some(DshSettingsLayout::LegacyYaml));
        assert_eq!(layout_for_version("0.2.0-rc.2"), Some(DshSettingsLayout::ProfilePatch));
        assert_eq!(layout_for_version("0.2.0"), Some(DshSettingsLayout::ProfilePatch));
        assert_eq!(layout_for_version("1.0.0"), Some(DshSettingsLayout::ProfilePatch));
        assert_eq!(layout_for_version("latest"), None);
        assert_eq!(layout_for_version(""), None);
    }

    // -- 一次性导入残留（settings.yaml.imported）的恢复 -----------------------
    //
    // 2026-10-08 真实案例的形状：dsh 0.2.0 的一次性导入把 settings.yaml 改名后
    // 逐节导入，strata 缺 api 导致整段 llm-pi-ai 被装配校验拒绝——5 个供应商
    // 全部「消失」，但数据完整留在 .imported 里。

    /// patch 文档里没有 llm-pi-ai entry（导入被拒后的真实样子）：别的 entry
    /// 都在，其中一条带 `!!js` 表达式——移植时它们一个比特都不能动。
    const PATCH_WITHOUT_PI_AI: &str = "\
# Your patch layer for this dsh profile, applied after every bundle layer:
- id: ui-theme
  name: \"@deepseek-ai/dsh-client-ui-theme\"
  config:
    preference: dark
- id: webserver
  config:
    port: !!js ctx.webStartup.port ?? 3080
- id: agent-default-model
  name: \"@deepseek-ai/dsh-agent-default-model\"
  config:
    provider: deepseek-official
    model: deepseek-flash
";

    /// 残留文件：一个目录路由（只有 key）、一个自定义路由（含启动器不编辑的
    /// `compat` 与块前注释），外加一行 providers 尾部的注释（不属于任何供应商，
    /// 移植时不能带走）。
    const IMPORTED_SAMPLE: &str = "\
ui-onboarding:
  welcomeNoticeVersion: 2026-08-13.1
llm-pi-ai:
  providers:
    kimi-coding:
      apiKeyEnv: KIMI_CODING_API_KEY
    # 自建网关
    vllm:
      apiKeyEnv: VLLM_API_KEY
      api: openai-completions
      baseURL: https://example.test/v1
      compat:
        thinkingFormat: qwen
      models:
        - id: Qwen3.8-27B
          reasoningEfforts:
            max: max
    # providers 尾部的注释
agent-default-model:
  provider: deepseek-official
  model: deepseek-flash
";

    #[test]
    fn restore_transplants_raw_blocks_into_patch_layout() {
        let (updated, restored, skipped) = restore_imported_from_text(
            PATCH_WITHOUT_PI_AI,
            IMPORTED_SAMPLE,
            DshSettingsLayout::ProfilePatch,
        )
        .expect("restore");

        assert_eq!(restored, vec!["kimi-coding", "vllm"]);
        assert!(skipped.is_empty());
        // entry 整块补齐在文件尾（agent-default-model 之后），形状与 dsh 自己的
        // configEditor 一致。
        let entry_at = updated.find("- id: llm-pi-ai").expect("entry");
        assert!(updated.find("- id: agent-default-model").expect("other") < entry_at);
        assert!(updated[entry_at..].starts_with(
            "- id: llm-pi-ai\n  name: '@deepseek-ai/dsh-llm-pi-ai'\n  config:\n    providers:\n"
        ), "实际 entry 起：{}", &updated[entry_at..]);
        // 供应商块平移到 key=6 / 字段=8：块前注释跟着走，字段逐字节保留（含
        // 启动器不编辑的 compat）。
        assert!(updated.contains("      kimi-coding:\n        apiKeyEnv: KIMI_CODING_API_KEY\n"), "实际：{updated}");
        assert!(updated.contains("      # 自建网关\n      vllm:\n        apiKeyEnv: VLLM_API_KEY\n"), "实际：{updated}");
        assert!(updated.contains("        compat:\n          thinkingFormat: qwen\n"), "实际：{updated}");
        assert!(updated.contains("        models:\n          - id: Qwen3.8-27B\n            reasoningEfforts:\n              max: max\n"), "实际：{updated}");
        // providers 尾部的注释不属于任何供应商，不能被带走。
        assert!(!updated.contains("providers 尾部的注释"), "实际：{updated}");
        // 其它 entry（含 !!js 表达式与文件头注释）一字节不动。
        assert!(updated.starts_with("# Your patch layer"));
        assert!(updated.contains("- id: webserver\n  config:\n    port: !!js ctx.webStartup.port ?? 3080\n"));
        assert!(updated.contains("- id: agent-default-model\n  name: \"@deepseek-ai/dsh-agent-default-model\"\n  config:\n    provider: deepseek-official\n    model: deepseek-flash\n"));
    }

    #[test]
    fn restore_into_legacy_layout_shifts_indent_down() {
        let current = "llm-pi-ai:\n  providers:\n    existing:\n      apiKeyEnv: EXISTING\n";
        let (updated, restored, skipped) = restore_imported_from_text(
            current,
            IMPORTED_SAMPLE,
            DshSettingsLayout::LegacyYaml,
        )
        .expect("restore");

        assert_eq!(restored, vec!["kimi-coding", "vllm"]);
        assert!(skipped.is_empty());
        // 旧布局 key=4 / 字段=6：往下平移两格。
        assert!(updated.contains("    kimi-coding:\n      apiKeyEnv: KIMI_CODING_API_KEY\n"), "实际：{updated}");
        assert!(updated.contains("    # 自建网关\n    vllm:\n      apiKeyEnv: VLLM_API_KEY\n      api: openai-completions\n"), "实际：{updated}");
        // 已有的块原样，新块追加在它后面。
        let existing_at = updated.find("    existing:\n      apiKeyEnv: EXISTING\n").expect("existing");
        assert!(existing_at < updated.find("    kimi-coding:").expect("kimi"));
    }

    #[test]
    fn restore_skips_providers_already_present() {
        // kimi-coding 已在当前文档里：它不进 restored 也不进 skipped，且全文
        // 仍然只有一份。
        let current = "\
- id: llm-pi-ai
  name: \"@deepseek-ai/dsh-llm-pi-ai\"
  config:
    providers:
      kimi-coding:
        apiKeyEnv: KIMI_CODING_API_KEY
";
        let (updated, restored, skipped) = restore_imported_from_text(
            current,
            IMPORTED_SAMPLE,
            DshSettingsLayout::ProfilePatch,
        )
        .expect("restore");

        assert_eq!(restored, vec!["vllm"]);
        assert!(skipped.is_empty());
        assert_eq!(updated.matches("kimi-coding:").count(), 1, "实际：{updated}");
    }

    #[test]
    fn restore_creates_the_entry_in_an_empty_document() {
        let (updated, restored, _) =
            restore_imported_from_text("", IMPORTED_SAMPLE, DshSettingsLayout::ProfilePatch)
                .expect("restore");
        assert_eq!(restored, vec!["kimi-coding", "vllm"]);
        assert!(updated.contains("- id: llm-pi-ai\n  name: '@deepseek-ai/dsh-llm-pi-ai'\n  config:\n    providers:\n      kimi-coding:"), "实际：{updated}");
    }

    #[test]
    fn restore_skips_custom_routes_missing_api() {
        // 缺 api 的自定义路由正是当初被 dsh 拒掉的那种：恢复时必须在 skipped 里
        // 说清原因，不能原样塞回去再丢一次。目录清单读不到时不做这条判定（那
        // 是正常状态），整条用例跳过。
        if installed_catalog_routes(DshSettingsLayout::ProfilePatch).is_none() {
            return;
        }
        let imported = "\
llm-pi-ai:
  providers:
    acme-broken:
      baseURL: https://acme.test/v1
      models:
        - id: m1
    acme-fine:
      api: openai-completions
      baseURL: https://acme2.test/v1
      models:
        - id: m2
";
        let (updated, restored, skipped) = restore_imported_from_text(
            PATCH_WITHOUT_PI_AI,
            imported,
            DshSettingsLayout::ProfilePatch,
        )
        .expect("restore");

        assert_eq!(restored, vec!["acme-fine"]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].provider.id, "acme-broken");
        assert!(skipped[0].problems.iter().any(|problem| problem.contains("wire 协议")), "实际：{:?}", skipped[0].problems);
        assert!(updated.contains("acme-fine:"), "实际：{updated}");
        assert!(!updated.contains("acme-broken"), "实际：{updated}");
    }

    #[test]
    fn scan_lists_only_missing_providers_with_their_problems() {
        let existing: std::collections::HashSet<&str> = ["kimi-coding"].into_iter().collect();
        let candidates =
            scan_imported_text(&existing, IMPORTED_SAMPLE, DshSettingsLayout::ProfilePatch)
                .expect("scan");
        assert_eq!(
            candidates.iter().map(|item| item.provider.id.as_str()).collect::<Vec<_>>(),
            vec!["vllm"]
        );
        // vllm 字段齐全（api/baseURL/models 都在），任何目录状态下都没有问题。
        assert!(candidates[0].problems.is_empty(), "实际：{:?}", candidates[0].problems);
    }

    #[test]
    fn scan_reports_unparseable_imported_file() {
        let existing: std::collections::HashSet<&str> = Default::default();
        assert!(scan_imported_text(
            &existing,
            "llm-pi-ai:\n\tproviders:\n",
            DshSettingsLayout::ProfilePatch
        )
        .is_err());
    }
}
