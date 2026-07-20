# OpenAI → Anthropic 格式转换说明

> 基于 `cc-switch` 项目（D:\Project\cc-switch）的实现分析

## 一、转换函数

### 核心入口

**文件：** `src-tauri/src/proxy/providers/transform.rs`

`openai_to_anthropic(body: Value) -> Result<Value>`

将 OpenAI Chat Completions 格式的响应体转换为 Anthropic Messages 格式。

### 其他转换函数

| 文件 | 函数 | 用途 |
|---|---|---|
| `transform.rs` | `anthropic_to_openai(body)` | Anthropic → OpenAI Chat Completions 请求 |
| `transform.rs` | `anthropic_to_openai_with_reasoning_content(body, preserve_reasoning_content)` | 同上，保留 reasoning_content |
| `transform_responses.rs` | `anthropic_to_responses(body)` | Anthropic → OpenAI Responses API 请求 |
| `transform_responses.rs` | `responses_to_anthropic(body)` | OpenAI Responses → Anthropic 响应 |
| `transform_gemini.rs` | `anthropic_to_gemini(body)` | Anthropic → Gemini 请求 |
| `transform_gemini.rs` | `gemini_to_anthropic(body)` | Gemini → Anthropic 响应 |
| `transform_codex_chat.rs` | 相关转换函数 | Anthropic ↔ OpenAI Codex Chat 请求 |

## 二、OpenAI → Anthropic 响应映射

### 字段对照表

| OpenAI 字段 | → Anthropic 字段 | 转换方式 |
|---|---|---|
| `choices[0].message.content`（字符串） | `content[{\type:"text", text:"..."}]` | 直接映射 |
| `choices[0].message.content`（数组） | `content[]` | 每个 text/output_text/refusal 块映射 |
| `choices[0].message.reasoning_content` | `content[{\type:"thinking", thinking:"..."}]` | reasoning 路径特殊处理 |
| `choices[0].message.tool_calls[]` | `content[{\type:"tool_use", id, name, input}]` | function.arguments 解析为 input object |
| `choices[0].message.function_call` | `content[{\type:"tool_use"}]` | 旧版格式兼容 |
| `choices[0].finish_reason` | `stop_reason` | 见下方映射表 |
| `usage.prompt_tokens` | `usage.input_tokens` | 减去 cache tokens |
| `usage.completion_tokens` | `usage.output_tokens` | 直接映射 |
| `usage.cache_read_input_tokens` | `usage.cache_read_input_tokens` | 缓存读取 token 保留 |
| `usage.cache_creation_input_tokens` | `usage.cache_creation_input_tokens` | 缓存创建 token 保留 |
| `usage.prompt_tokens_details.cached_tokens` | `usage.cache_read_input_tokens` | 兼容旧版 cache token 格式 |
| `id` | `id` | 透传 |

### finish_reason → stop_reason 映射

```rust
match finish_reason {
    "stop"          => "end_turn",
    "length"        => "max_tokens",
    "tool_calls"    => "tool_use",
    "function_call" => "tool_use",
    "content_filter"=> "end_turn",
    _               => "end_turn",
}
```

### tool_calls → tool_use 映射

```rust
// OpenAI tool_calls 数组 → Anthropic content blocks
for tool_call in choices[0].message.tool_calls {
    let input = tool_call.function.arguments.parse::<Value>()?;
    // → {type: "tool_use", id: tool_call.id, name: tool_call.function.name, input}
}
```

## 三、Anthropic → OpenAI 请求映射

### 核心映射表

| Anthropic 字段 | OpenAI 字段 | 转换方式 |
|---|---|---|
| `system`（字符串或数组） | `messages[0].content`（role=system） | 合并为单个系统消息；剥离 billing header |
| `messages[].content`（字符串） | `messages[].content`（字符串） | 直接透传 |
| `messages[].content[{type:"text"}]` | `messages[].content`（字符串或数组） | 单 text 块简化，多 block 保留数组 |
| `messages[].content[{type:"image"}]` | `messages[].content[{type:"image_url"}]` | Base64 data URL 格式 |
| `messages[].content[{type:"tool_use"}]` | `messages[].tool_calls` | tool_use → tool_calls (function format) |
| `messages[].content[{type:"tool_result"}]` | 新消息 (role=tool) | 独立 tool 角色消息 |
| `messages[].content[{type:"thinking"}]` | 默认丢弃 | 保留时作为 reasoning_content |
| `max_tokens` | `max_tokens`（o 系列 → `max_completion_tokens`） | 条件重命名 |
| `temperature` | `temperature` | 透传 |
| `top_p` | `top_p` | 透传 |
| `stop_sequences` | `stop` | 数组重命名 |
| `tools[{input_schema}]` | `tools[{parameters}]` | input_schema → parameters |
| `tool_choice` | `tool_choice` | "any" → "required" |
| `thinking` / `output_config.effort` | `reasoning_effort` | 仅 o 系列 / GPT-5+ 模型 |

## 四、OpenAI Responses API ↔ Anthropic

### Anthropic → Responses API

| Anthropic 字段 | Responses API 字段 | 转换方式 |
|---|---|---|
| `system` | `instructions` | 字符串或数组拼接 |
| text block | `{type:"input_text"/"output_text"}` | role=user → input_text, role=assistant → output_text |
| image block | `{type:"input_image"}` | Base64 data URL |
| tool_use block | `{type:"function_call"}` | 提升到顶层 input 项 |
| tool_result block | `{type:"function_call_output"}` | 提升到顶层 input 项 |
| thinking block | （丢弃） | Responses API 不支持 |
| `max_tokens` | `max_output_tokens` | 字段重命名 |

### Responses API → Anthropic

| Responses API 字段 | Anthropic 字段 | 转换方式 |
|---|---|---|
| `output[].{type:"message", content[]}` | `content[]` | output_text → text, refusal → text |
| `output[].{type:"function_call"}` | `content[{type:"tool_use"}]` | call_id → id, arguments → input |
| `output[].{type:"reasoning"}` | `content[{type:"thinking"}]` | summary 数组拼接 |
| `status` | `stop_reason` | completed → end_turn/tool_use, incomplete → max_tokens |

## 五、架构设计

### ProviderAdapter 接口

```rust
trait ProviderAdapter {
    fn transform_request(&self, body: Value) -> Result<Value>;
    fn transform_response(&self, body: Value) -> Result<Value>;
}
```

### ClaudeAdapter 分发逻辑

**请求路径** — 根据 `api_format` 分发到对应转换器：
- `"anthropic"` → 透传（不转换）
- `"openai_chat"` → `anthropic_to_openai_with_reasoning_content()`
- `"openai_responses"` → `anthropic_to_responses()`
- `"gemini_native"` → `anthropic_to_gemini_with_shadow()`

**响应路径** — 启发式检测 body 中的关键字段选择转换器：
- 有 `candidates` 或 `promptFeedback` → Gemini → Anthropic
- 有 `output` → Responses → Anthropic
- 其他 → Chat Completions → Anthropic

### 文件结构

```
src-tauri/src/proxy/providers/
├── adapter.rs              # ProviderAdapter trait 定义
├── claude.rs               # ClaudeAdapter 实现（分发逻辑）
├── mod.rs                  # ProviderType 枚举、needs_transform()
├── models/
│   ├── anthropic.rs        # Anthropic 请求/响应数据结构
│   └── openai.rs           # OpenAI 请求/响应数据结构
├── transform.rs            # OpenAI Chat Completions 双向转换
├── transform_responses.rs  # OpenAI Responses API 双向转换
└── transform_gemini.rs     # Gemini 双向转换
```
