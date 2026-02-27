# 运行时配置注入

> AgentConfig、ConfigProvider trait、WASM 配置传递、Builder 集成

## 1. 设计动机

AI Agent 的运行时参数（API key、模型名、base URL、自定义 headers 等）**不应编译进二进制**，尤其是：

- **WASM Guest 模块**——`.wasm` 文件是可分发的插件，API key 等敏感数据必须由宿主在运行时注入
- **多环境部署**——同一个 Agent 二进制在 dev / staging / prod 使用不同的 endpoint 和 key
- **动态密钥轮换**——API key 可能从 secrets manager 动态获取，不能写死
- **共享配置**——AgentLoop 内的 Model、Tool、Adapter 可能共享同一份配置（如 base_url）

## 2. AgentConfig（config.rs）

标准化的配置容器，JSON 可序列化（跨 WASM 边界传递）：

```rust
use std::collections::HashMap;

/// Agent 运行时配置——所有参数均为 Option，按需设置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    // ── 模型相关 ──
    /// API key（OpenAI、Azure 等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// 模型名称（如 "gpt-4o", "claude-3-opus"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// API base URL（如 "https://api.openai.com/v1"）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    // ── 请求参数默认值 ──
    /// 默认 temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// 默认 max_tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    // ── 传输相关 ──
    /// 自定义 HTTP headers（如 Authorization 覆盖、X-Api-Version 等）
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// 请求超时（毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    // ── 扩展 ──
    /// 自定义扩展配置（Tool 专用参数、业务参数等）
    /// 框架不解释，由 Tool / Adapter / 用户代码自行读取
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}
```

### AgentConfig Builder 风格构造

```rust
impl AgentConfig {
    pub fn new() -> Self { Self::default() }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into()); self
    }
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into()); self
    }
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into()); self
    }
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.temperature = Some(temp); self
    }
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n); self
    }
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into()); self
    }
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = Some(ms); self
    }
    pub fn with_extra(mut self, extra: serde_json::Value) -> Self {
        self.extra = extra; self
    }

    /// 从环境变量加载常见配置
    /// REMI_API_KEY, REMI_MODEL, REMI_BASE_URL, REMI_TEMPERATURE, REMI_MAX_TOKENS, REMI_TIMEOUT_MS
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_env() -> Self {
        Self {
            api_key: std::env::var("REMI_API_KEY").ok(),
            model: std::env::var("REMI_MODEL").ok(),
            base_url: std::env::var("REMI_BASE_URL").ok(),
            temperature: std::env::var("REMI_TEMPERATURE").ok()
                .and_then(|s| s.parse().ok()),
            max_tokens: std::env::var("REMI_MAX_TOKENS").ok()
                .and_then(|s| s.parse().ok()),
            timeout_ms: std::env::var("REMI_TIMEOUT_MS").ok()
                .and_then(|s| s.parse().ok()),
            ..Default::default()
        }
    }

    /// 合并：other 的非 None 字段覆盖 self
    pub fn merge(mut self, other: &AgentConfig) -> Self {
        if other.api_key.is_some() { self.api_key = other.api_key.clone(); }
        if other.model.is_some() { self.model = other.model.clone(); }
        if other.base_url.is_some() { self.base_url = other.base_url.clone(); }
        if other.temperature.is_some() { self.temperature = other.temperature; }
        if other.max_tokens.is_some() { self.max_tokens = other.max_tokens; }
        if other.timeout_ms.is_some() { self.timeout_ms = other.timeout_ms; }
        for (k, v) in &other.headers { self.headers.insert(k.clone(), v.clone()); }
        if !other.extra.is_null() { self.extra = other.extra.clone(); }
        self
    }
}
```

## 3. ConfigProvider trait

对于简单场景，直接传入 `AgentConfig` 即可。对于需要动态解析的场景（密钥轮换、多租户等），提供 `ConfigProvider` trait：

```rust
/// 动态配置提供者——用于运行时解析配置
/// 例如：从 secrets manager 获取最新 API key
pub trait ConfigProvider {
    /// 获取当前配置
    /// 每次 agent.chat() 调用时会调用一次，获取最新配置
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>>;
}

// AgentConfig 自身就是最简单的 ConfigProvider（静态配置）
impl ConfigProvider for AgentConfig {
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>> {
        async { Ok(self.clone()) }
    }
}
```

### 动态 ConfigProvider 示例

```rust
/// 从环境变量动态读取（每次调用重新读取）
struct EnvConfigProvider;

impl ConfigProvider for EnvConfigProvider {
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>> {
        async { Ok(AgentConfig::from_env()) }
    }
}

/// 多租户：根据请求上下文选择不同 API key
struct MultiTenantConfig {
    configs: HashMap<String, AgentConfig>,
    default: AgentConfig,
}

impl ConfigProvider for MultiTenantConfig {
    fn resolve(&self) -> impl Future<Output = Result<AgentConfig, AgentError>> {
        async { Ok(self.default.clone()) }
    }
}

impl MultiTenantConfig {
    /// 按租户 ID 解析
    pub fn resolve_for_tenant(&self, tenant_id: &str) -> Result<AgentConfig, AgentError> {
        self.configs.get(tenant_id)
            .cloned()
            .ok_or_else(|| AgentError::Model(format!("Unknown tenant: {tenant_id}")))
    }
}
```

## 4. OpenAIClient 集成

`OpenAIClient` 支持从 `AgentConfig` 构造，也支持在每次 `chat()` 调用时动态应用配置覆盖：

```rust
impl OpenAIClient {
    /// 从 AgentConfig 构造（WASM 友好——不需要编译时确定 key）
    pub fn from_config(config: &AgentConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: config.base_url.clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            api_key: config.api_key.clone().unwrap_or_default(),
            model: config.model.clone().unwrap_or_else(|| "gpt-4o".into()),
        }
    }

    /// 用运行时配置覆盖请求参数
    /// AgentLoop 在每次 model.chat() 前调用此方法
    pub(crate) fn apply_config_overrides(req: &mut ChatRequest, config: &AgentConfig) {
        if let Some(model) = &config.model {
            req.model = model.clone();
        }
        if let Some(temp) = config.temperature {
            req.temperature = Some(temp);
        }
        if let Some(max) = config.max_tokens {
            req.max_tokens = Some(max);
        }
    }
}
```

## 5. AgentBuilder 集成

`AgentBuilder` 新增 `.config()` 和 `.config_provider()` 方法：

```rust
pub struct AgentBuilder<M, S = NoStore> {
    model: M,
    store: S,
    config: Option<AgentConfig>,           // 静态配置
    system_prompt: Option<String>,
    tools: ToolRegistry,
    max_turns: usize,
}

impl<M: ChatModel, S> AgentBuilder<M, S> {
    /// 注入运行时配置（API key、model 覆盖、自定义 headers 等）
    pub fn config(mut self, config: AgentConfig) -> Self {
        self.config = Some(config);
        self
    }
}

// BuiltAgent 持有配置
pub struct BuiltAgent<M: ChatModel, S = NoStore> {
    model: M,
    store: S,
    config: AgentConfig,           // 始终存在（默认为 Default）
    system_prompt: String,
    tools: ToolRegistry,
    max_turns: usize,
}
```

### 配置传导路径

```
AgentConfig
    │
    ├──→ OpenAIClient::from_config()      // 构造 model 客户端
    │
    ├──→ AgentLoop（每次 model.chat() 前）
    │    └─ apply_config_overrides(&mut req, &config)
    │       • model 名
    │       • temperature / max_tokens 默认值
    │
    ├──→ Tool::execute() via ToolContext   // Tool 可读取 extra 配置
    │    └─ tool 可访问 config.extra 获取自定义参数
    │
    └──→ Transport headers                // HTTP 请求自定义 headers
         └─ config.headers 追加到 reqwest 请求
```

## 6. Tool 访问配置

Tool 有时需要运行时配置（如外部 API key、数据库连接字符串等）。通过在 `execute()` 参数中传入配置引用即可：

```rust
/// Tool 执行时的上下文，由 AgentLoop 注入
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// 当前 agent 的运行时配置
    pub config: AgentConfig,
    /// 当前 Thread ID（如有）
    pub thread_id: Option<ThreadId>,
    /// 当前 Run ID
    pub run_id: RunId,
    /// 请求携带的业务自定义 metadata（透传）
    pub metadata: Option<serde_json::Value>,
}

pub trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;

    /// 执行工具——可选地接收 ToolContext
    /// 默认签名不变（向后兼容），需要上下文时实现 execute_with_context
    fn execute(
        &self,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>;

    /// 带上下文的执行（默认委托到 execute）
    fn execute_with_context(
        &self,
        arguments: serde_json::Value,
        _ctx: &ToolContext,
    ) -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>> {
        self.execute(arguments)
    }
}
```

### Tool 读取 extra 配置示例

```rust
struct WeatherTool;

impl Tool for WeatherTool {
    fn name(&self) -> &str { "get_weather" }
    fn description(&self) -> &str { "Get current weather" }
    fn parameters_schema(&self) -> serde_json::Value { /* ... */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        // 无上下文时使用默认行为
        async move { Ok(ToolResult::Output(stream! { yield ToolOutput::Result("N/A".into()); })) }
    }

    fn execute_with_context(&self, args: serde_json::Value, ctx: &ToolContext)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            // 从 config.extra 读取天气 API key
            let weather_api_key = ctx.config.extra["weather_api_key"]
                .as_str()
                .ok_or_else(|| AgentError::ToolExecution {
                    tool_name: "get_weather".into(),
                    message: "Missing weather_api_key in config.extra".into(),
                })?;

            Ok(ToolResult::Output(stream! {
                let city = args["city"].as_str().unwrap_or("Tokyo");
                yield ToolOutput::Delta(format!("Fetching weather for {city}..."));
                // 使用 weather_api_key 调用外部 API
                let result = fetch_weather(weather_api_key, city).await;
                yield ToolOutput::Result(result);
            }))
        }
    }
}
```

## 7. WASM 配置传递

WASM Guest 模块不能访问环境变量，也不应编译进 API key。配置必须由宿主在运行时注入。

### 7.1 Guest 端：set_config 导出函数

```rust
// guest/exports.rs 新增导出

/// 宿主注入配置（在 chat() 调用前执行）
/// 宿主先 alloc() 写入 AgentConfig JSON，然后调用此函数
#[no_mangle]
pub extern "C" fn set_config(config_ptr: u32, config_len: u32) -> u32 {
    // 读取 JSON，解析为 AgentConfig
    // 存入 global: RefCell<Option<AgentConfig>>
    // 返回 0 = 成功，非 0 = 错误
    let bytes = unsafe { read_memory(config_ptr, config_len) };
    match serde_json::from_slice::<AgentConfig>(&bytes) {
        Ok(config) => {
            GLOBAL_CONFIG.with(|c| c.replace(Some(config)));
            0
        }
        Err(_) => 1,
    }
}

thread_local! {
    static GLOBAL_CONFIG: RefCell<Option<AgentConfig>> = RefCell::new(None);
}

/// Guest 内部获取当前配置
pub fn get_config() -> AgentConfig {
    GLOBAL_CONFIG.with(|c| {
        c.borrow().clone().unwrap_or_default()
    })
}
```

### 7.2 Host 端：WasmAgent 配置注入

```rust
impl WasmAgent {
    /// 带配置加载
    pub fn from_bytes_with_config(
        wasm_bytes: &[u8],
        config: AgentConfig,
    ) -> Result<Self, ProtocolError> {
        let mut agent = Self::from_bytes(wasm_bytes)?;
        agent.config = Some(config);
        Ok(agent)
    }

    /// 运行时更新配置（如密钥轮换）
    pub fn set_config(&mut self, config: AgentConfig) {
        self.config = Some(config);
    }
}

// 在 chat() 实现中，创建 Instance 后注入配置
impl Agent for WasmAgent {
    fn chat(&self, req: ProtocolRequest)
        -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>>
    {
        async move {
            let mut store = wasmi::Store::new(&self.engine, ());
            let instance = self.module.instantiate(&mut store, ...)?;

            // ── 注入配置 ──
            if let Some(config) = &self.config {
                let config_json = serde_json::to_vec(config)?;
                let alloc_fn = instance.get_typed_func::<u32, u32>(&store, "alloc")?;
                let ptr = alloc_fn.call(&mut store, config_json.len() as u32)?;
                write_guest_memory(&mut store, &instance, ptr, &config_json);

                let set_config_fn = instance
                    .get_typed_func::<(u32, u32), u32>(&store, "set_config")?;
                let result = set_config_fn.call(
                    &mut store, (ptr, config_json.len() as u32)
                )?;
                if result != 0 {
                    return Err(ProtocolError {
                        code: "config_error".into(),
                        message: "Failed to set config in WASM guest".into(),
                    });
                }
            }

            // ── 正常 chat 流程 ──
            // ...（同 08-transport.md 中的现有逻辑）

            Ok(stream! { /* ... */ })
        }
    }
}
```

### 7.3 WASM Guest 使用配置

```rust
// WASM Guest 内的 Agent 实现
struct MyWasmAgent;

impl Agent for MyWasmAgent {
    type Request = ProtocolRequest;
    type Response = ProtocolEvent;
    type Error = ProtocolError;

    fn chat(&self, req: ProtocolRequest)
        -> impl Future<Output = Result<impl Stream<Item = ProtocolEvent>, ProtocolError>>
    {
        async move {
            // 从宿主注入的配置中获取 API key 和 model
            let config = guest::get_config();
            let api_key = config.api_key
                .ok_or_else(|| ProtocolError {
                    code: "missing_config".into(),
                    message: "api_key not provided by host".into(),
                })?;

            // 用配置构造 model client
            let model = OpenAIClient::from_config(&config);

            // 正常使用 model
            let mut stream = model.chat(ChatRequest {
                model: config.model.unwrap_or("gpt-4o".into()),
                messages: req.messages,
                ..Default::default()
            }).await.map_err(|e| ProtocolError::from(e))?;

            Ok(stream! {
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        ChatResponseChunk::Delta { content, .. } => {
                            yield ProtocolEvent::Delta {
                                content,
                                role: Some("assistant".into()),
                            };
                        }
                        ChatResponseChunk::Done => {
                            yield ProtocolEvent::Done;
                        }
                        _ => {}
                    }
                }
            })
        }
    }
}
```

## 8. 端到端使用示例

### 8.1 Native：从环境变量配置

```rust
#[tokio::main]
async fn main() -> Result<(), AgentError> {
    // 从环境变量加载（REMI_API_KEY, REMI_MODEL 等）
    let config = AgentConfig::from_env();

    let agent = AgentBuilder::new()
        .model(OpenAIClient::from_config(&config))
        .config(config)
        .system("You are helpful.")
        .tool(WeatherTool)
        .build();

    let mut stream = agent.chat("What's the weather?".into()).await?;
    // ...
    Ok(())
}
```

### 8.2 WASM 宿主：注入配置到插件

```rust
fn main() -> Result<(), ProtocolError> {
    let config = AgentConfig::new()
        .with_api_key("sk-prod-xxx")
        .with_model("gpt-4o")
        .with_base_url("https://api.openai.com/v1")
        .with_extra(serde_json::json!({
            "weather_api_key": "weather-api-xxx",
            "max_retries": 3
        }));

    let plugin = WasmAgent::from_bytes_with_config(
        include_bytes!("plugins/weather_agent.wasm"),
        config,
    )?;

    // plugin 是一个普通的 impl Agent，可继续组合
    let agent = plugin.map_response(|e| /* ... */);
    // ...
    Ok(())
}
```

### 8.3 HTTP Server：从请求头注入配置

```rust
// 中间件层：从 HTTP 请求提取租户配置
async fn tenant_config_middleware(
    headers: HeaderMap,
    mut req: ProtocolRequest,
) -> ProtocolRequest {
    // 从 Authorization header 解析租户
    // 查询配置数据库获取该租户的 API key
    // 注入到请求的 extra 字段
    req
}
```

## 9. InterruptHandler——自动中断处理器

结合 Config 和 Interrupt 自动处理，提供 `InterruptHandler` trait 让应用层按 `kind` 自动路由：

```rust
/// 中断自动处理器——按 kind 路由到对应处理逻辑
pub trait InterruptHandler {
    /// 是否能处理此类中断
    fn can_handle(&self, kind: &str) -> bool;

    /// 处理中断，返回 resume payload
    fn handle(
        &self,
        info: &InterruptInfo,
    ) -> impl Future<Output = Result<serde_json::Value, AgentError>>;
}

/// 中断处理路由器——注册多个 handler，按 kind 自动分发
pub struct InterruptRouter {
    handlers: Vec<Box<dyn DynInterruptHandler>>,
}

impl InterruptRouter {
    pub fn new() -> Self { Self { handlers: Vec::new() } }

    pub fn register(mut self, handler: impl InterruptHandler + 'static) -> Self {
        self.handlers.push(Box::new(handler));
        self
    }

    /// 尝试自动处理所有中断
    /// 返回 (已处理的 payloads, 无法自动处理的 interrupts)
    pub async fn try_handle_all(
        &self,
        interrupts: &[InterruptInfo],
    ) -> (Vec<ResumePayload>, Vec<InterruptInfo>) {
        let mut payloads = Vec::new();
        let mut unhandled = Vec::new();

        for info in interrupts {
            if let Some(handler) = self.handlers.iter()
                .find(|h| h.can_handle(&info.kind))
            {
                match handler.handle(info).await {
                    Ok(result) => payloads.push(ResumePayload {
                        interrupt_id: info.interrupt_id.clone(),
                        result,
                    }),
                    Err(_) => unhandled.push(info.clone()),
                }
            } else {
                unhandled.push(info.clone());
            }
        }

        (payloads, unhandled)
    }
}
```

### 自动处理示例

```rust
/// 策略检查：金额 < 100 自动批准
struct AutoApprovalPolicy;

impl InterruptHandler for AutoApprovalPolicy {
    fn can_handle(&self, kind: &str) -> bool {
        kind == "payment_approval"
    }

    fn handle(&self, info: &InterruptInfo)
        -> impl Future<Output = Result<serde_json::Value, AgentError>>
    {
        async move {
            let amount = info.data["amount"].as_f64().unwrap_or(f64::MAX);
            if amount < 100.0 {
                Ok(serde_json::json!({ "approved": true, "reason": "auto: amount < 100" }))
            } else {
                Err(AgentError::Model("Amount too large for auto-approval".into()))
            }
        }
    }
}

/// 使用 InterruptRouter 自动 + 人工混合处理
let router = InterruptRouter::new()
    .register(AutoApprovalPolicy)
    .register(RateLimitWaitHandler);  // rate_limit_wait 类型自动等待重试

// 在 agent loop 消费中
while let Some(event) = stream.next().await {
    match event {
        AgentEvent::Interrupt { interrupts } => {
            let (auto_payloads, manual_interrupts) =
                router.try_handle_all(&interrupts).await;

            if manual_interrupts.is_empty() {
                // 全部自动处理——直接 resume
                stream = agent.resume_run(&thread_id, &run_id, auto_payloads).await?;
            } else {
                // 部分需要人工——等待人工输入后合并 resume
                let manual_payloads = wait_for_human_input(&manual_interrupts).await;
                let all_payloads = [auto_payloads, manual_payloads].concat();
                stream = agent.resume_run(&thread_id, &run_id, all_payloads).await?;
            }
        }
        // ...
    }
}
```

## 10. AgentBuilder 集成 InterruptRouter

```rust
impl<M: ChatModel, S> AgentBuilder<M, S> {
    /// 注入中断自动处理路由器
    pub fn interrupt_router(mut self, router: InterruptRouter) -> Self {
        self.interrupt_router = Some(router);
        self
    }
}
```

BuiltAgent 在 `chat_in_thread()` 返回的 stream 中，当遇到 Interrupt 且配置了 router 时，自动尝试处理。无法自动处理的 interrupt 仍然 yield 给调用方。

## 11. 模块结构更新

```
src/
├── config.rs           # AgentConfig, ConfigProvider trait  ← NEW
├── interrupt.rs        # InterruptHandler, InterruptRouter  ← NEW
├── tool/
│   ├── mod.rs          # Tool trait (含 execute_with_context), ToolContext
│   └── ...
├── builder.rs          # AgentBuilder (含 .config(), .interrupt_router())
└── ...
```
