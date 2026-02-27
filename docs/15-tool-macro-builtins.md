# Tool 宏 + 内置 Tool

> `#[tool]` 过程宏简化 Tool 创建、内置 Bash / FS / 常用 Tool

## 1. 设计动机

当前 `Tool` trait 手动实现存在大量 boilerplate：

```rust
// 现状：每个 tool 需要手写 4 个方法
impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }                     // 冗余
    fn description(&self) -> &str { "Does something" }        // 冗余
    fn parameters_schema(&self) -> serde_json::Value { ... }  // 手写 JSON Schema 易出错
    fn execute(&self, args: serde_json::Value) -> ... { ... }
}
```

痛点：
- **name / description 冗余**——信息已经存在于函数命名和文档注释中
- **手写 JSON Schema 易出错**——参数类型/约束必须手动拼 JSON，与 Rust 类型不同步
- **缺少常用 tool**——每个用户都要自己实现 bash 执行、文件读写等基础工具

## 2. `#[tool]` 过程宏

### 2.1 基本用法

将普通 async 函数标注为 Tool。宏自动生成 `Tool` trait impl，从函数签名推导 JSON Schema，从 doc comment 提取 description：

```rust
use remi_agentloop::tool;

/// Search the web for information
#[tool]
async fn web_search(
    /// The search query string
    query: String,
    /// Maximum number of results to return
    #[tool(default = 10)]
    max_results: Option<u32>,
) -> Result<String, AgentError> {
    let results = do_search(&query, max_results.unwrap_or(10)).await;
    Ok(results)
}
```

宏展开后等价于：

```rust
struct WebSearch;

impl Tool for WebSearch {
    fn name(&self) -> &str { "web_search" }

    fn description(&self) -> &str { "Search the web for information" }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query string"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 10
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, arguments: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let query: String = serde_json::from_value(arguments["query"].clone())
                .map_err(|e| AgentError::ToolExecution {
                    tool_name: "web_search".into(),
                    message: format!("Invalid 'query': {e}"),
                })?;
            let max_results: Option<u32> = arguments.get("max_results")
                .and_then(|v| serde_json::from_value(v.clone()).ok());

            let result = web_search_impl(query, max_results).await?;

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Result(result);
            }))
        }
    }
}

// 原始函数保留为内部实现
async fn web_search_impl(query: String, max_results: Option<u32>) -> Result<String, AgentError> {
    // ... 原始函数体
}
```

### 2.2 流式返回

函数返回 `Stream` 时，宏生成流式 Tool（自动包装为 `ToolResult::Output`）：

```rust
/// Analyze data and report progress
#[tool]
async fn analyze_data(
    /// JSON data to analyze
    data: serde_json::Value,
) -> impl Stream<Item = ToolOutput> {
    stream! {
        yield ToolOutput::Delta("Starting analysis...".into());
        let result = heavy_computation(&data).await;
        yield ToolOutput::Result(result);
    }
}
```

### 2.3 带 ToolContext

函数参数中包含 `ctx: &ToolContext` 时，宏自动生成 `execute_with_context` 而非 `execute`：

```rust
/// Get weather for a city using the configured API key
#[tool]
async fn get_weather(
    /// City name
    city: String,
    ctx: &ToolContext,
) -> Result<String, AgentError> {
    let api_key = ctx.config.extra["weather_api_key"]
        .as_str()
        .ok_or(AgentError::ToolExecution {
            tool_name: "get_weather".into(),
            message: "Missing weather_api_key".into(),
        })?;
    let result = fetch_weather(api_key, &city).await;
    Ok(result)
}
```

### 2.4 带 Interrupt

使用 `#[tool(interrupt)]` 标记可能中断的 tool。函数返回 `ToolResult<impl Stream<Item = ToolOutput>>`，根据业务逻辑决定走 `Output`（流式执行）还是 `Interrupt`（中断请求）路径：

```rust
/// Process a payment, requires approval for amounts > $100
#[tool(interrupt)]
async fn process_payment(
    /// Payment amount in dollars
    amount: f64,
    /// Payment description
    description: String,
) -> ToolResult<impl Stream<Item = ToolOutput>> {
    if amount > 100.0 {
        // 超额——返回 Interrupt，无 stream
        ToolResult::Interrupt(InterruptRequest {
            interrupt_id: InterruptId::new(),
            kind: "payment_approval".into(),
            data: serde_json::json!({ "amount": amount, "description": description }),
        })
    } else {
        // 小额——直接执行，返回 stream
        ToolResult::Output(stream! {
            yield ToolOutput::Result(format!("Payment of ${amount} processed"));
        })
    }
}
```

> **类型分离**：`#[tool(interrupt)]` 生成的 `execute` 返回 `Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>`。`ToolResult::Interrupt` 路径不包含任何 stream，与 `ToolResult::Output` 在类型层面彻底分离。

### 2.5 自定义名称 / 描述

```rust
/// This doc comment is ignored when name/description are explicit
#[tool(name = "run_query", description = "Execute a SQL query against the database")]
async fn execute_sql_query(
    /// SQL query string
    sql: String,
) -> Result<String, AgentError> {
    // ...
}
```

### 2.6 Enum 参数

Rust enum（带 `#[derive(Deserialize)]`）自动映射为 JSON Schema `enum`：

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SortOrder {
    Ascending,
    Descending,
}

/// Sort a list of items
#[tool]
async fn sort_items(
    /// Items to sort
    items: Vec<String>,
    /// Sort direction
    order: SortOrder,
) -> Result<String, AgentError> {
    // ...
}

// 生成的 schema:
// "order": { "type": "string", "enum": ["ascending", "descending"], "description": "Sort direction" }
```

### 2.7 类型到 JSON Schema 映射规则

| Rust 类型 | JSON Schema type | 备注 |
|----------|-----------------|------|
| `String` | `"string"` | |
| `&str` | `"string"` | 宏展开时转为 owned |
| `i32` / `i64` / `u32` / `u64` | `"integer"` | |
| `f32` / `f64` | `"number"` | |
| `bool` | `"boolean"` | |
| `Vec<T>` | `{ "type": "array", "items": T }` | 递归 |
| `Option<T>` | T 的 schema | 从 `required` 移除 |
| `serde_json::Value` | `{}` (any) | 不限制 |
| `#[derive(Deserialize)] struct` | `"object"` + 递归 properties | 嵌套对象 |
| `#[derive(Deserialize)] enum` | `"string"` + `enum` 值列表 | serde rename_all 感知 |

### 2.8 宏注册到 Builder

宏生成的 Tool 是零大小类型（ZST），直接注册到 Builder：

```rust
let agent = AgentBuilder::new()
    .model(model)
    .tool(WebSearch)         // 宏生成的 ZST
    .tool(GetWeather)        // 宏生成的 ZST
    .tool(ProcessPayment)    // 宏生成的 ZST
    .build();
```

### 2.9 实现方案

使用 proc-macro crate（`remi-agentloop-macros`），主 crate 通过 re-export 提供：

```
remi-agentloop/
├── Cargo.toml              # 主 crate，依赖 remi-agentloop-macros
├── macros/
│   ├── Cargo.toml          # proc-macro = true
│   └── src/
│       └── lib.rs          # #[tool] 宏实现
└── src/
    └── lib.rs              # pub use remi_agentloop_macros::tool;
```

```toml
# macros/Cargo.toml
[lib]
proc-macro = true

[dependencies]
syn = { version = "2", features = ["full"] }
quote = "1"
proc-macro2 = "1"
```

宏处理步骤：
1. 解析函数签名（`syn::ItemFn`）
2. 提取 doc comment → `description`
3. 函数名 → `name`（snake_case）
4. 遍历参数：跳过 `&ToolContext`，其余映射为 JSON Schema properties
5. `Option<T>` 参数不加入 `required`
6. 生成 `struct` + `impl Tool` + 保留原始函数体

---

## 3. 内置 Tool

框架提供常用 tool，通过 feature flag 按需启用：

```toml
[features]
tool-bash = []                        # BashTool
tool-fs = []                          # FsTool (physical)
tool-fs-virtual = []                  # VirtualFsTool (sandboxed)
tools = ["tool-bash", "tool-fs"]      # 全部内置 tool
```

### 3.1 BashTool——Shell 命令执行

安全的 shell 命令执行工具，支持超时、工作目录、环境变量注入。

```rust
/// Execute a bash command and return its output.
///
/// The command runs in a subprocess with configurable timeout,
/// working directory, and environment variables.
/// Stderr is captured separately. Exit code is included in the result.
pub struct BashTool {
    /// 允许的最大执行时间（默认 30s）
    timeout: Duration,
    /// 工作目录（默认当前目录）
    working_dir: Option<PathBuf>,
    /// 注入的环境变量
    env: HashMap<String, String>,
    /// 命令白名单（None = 不限制）
    allowed_commands: Option<Vec<String>>,
    /// 命令黑名单
    denied_commands: Vec<String>,
    /// 最大输出长度（字节，默认 1MB）
    max_output_bytes: usize,
}
```

#### Tool Schema

```json
{
  "name": "bash",
  "description": "Execute a bash command and return stdout/stderr/exit_code",
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The bash command to execute"
      },
      "working_dir": {
        "type": "string",
        "description": "Optional working directory for the command"
      },
      "timeout_secs": {
        "type": "integer",
        "description": "Optional timeout in seconds (default: 30)"
      }
    },
    "required": ["command"]
  }
}
```

#### 实现

```rust
impl BashTool {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            working_dir: None,
            env: HashMap::new(),
            allowed_commands: None,
            denied_commands: vec![
                "rm -rf /".into(),
                "mkfs".into(),
                "dd if=/dev".into(),
            ],
            max_output_bytes: 1_048_576, // 1MB
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout; self
    }
    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into()); self
    }
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into()); self
    }
    pub fn allow_only(mut self, commands: Vec<String>) -> Self {
        self.allowed_commands = Some(commands); self
    }
    pub fn deny(mut self, pattern: impl Into<String>) -> Self {
        self.denied_commands.push(pattern.into()); self
    }
    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes; self
    }

    /// 安全检查——命令是否合法
    fn validate_command(&self, cmd: &str) -> Result<(), AgentError> {
        // 黑名单检查
        for denied in &self.denied_commands {
            if cmd.contains(denied.as_str()) {
                return Err(AgentError::ToolExecution {
                    tool_name: "bash".into(),
                    message: format!("Command denied: contains '{denied}'"),
                });
            }
        }
        // 白名单检查
        if let Some(allowed) = &self.allowed_commands {
            let first_word = cmd.split_whitespace().next().unwrap_or("");
            if !allowed.iter().any(|a| a == first_word) {
                return Err(AgentError::ToolExecution {
                    tool_name: "bash".into(),
                    message: format!("Command not allowed: '{first_word}'"),
                });
            }
        }
        Ok(())
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str {
        "Execute a bash command and return stdout/stderr/exit_code"
    }
    fn parameters_schema(&self) -> serde_json::Value { /* 见上方 */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let command = args["command"].as_str()
                .ok_or(AgentError::ToolExecution {
                    tool_name: "bash".into(),
                    message: "Missing 'command' parameter".into(),
                })?
                .to_string();

            self.validate_command(&command)?;

            let timeout = args.get("timeout_secs")
                .and_then(|v| v.as_u64())
                .map(Duration::from_secs)
                .unwrap_or(self.timeout);

            let working_dir = args.get("working_dir")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .or_else(|| self.working_dir.clone());

            Ok(ToolResult::Output(stream! {
                yield ToolOutput::Delta(format!("$ {command}\n"));

                let mut cmd = tokio::process::Command::new("bash");
                cmd.arg("-c").arg(&command);

                if let Some(dir) = &working_dir {
                    cmd.current_dir(dir);
                }
                for (k, v) in &self.env {
                    cmd.env(k, v);
                }

                cmd.stdout(std::process::Stdio::piped());
                cmd.stderr(std::process::Stdio::piped());

                let result = tokio::time::timeout(timeout, async {
                    let output = cmd.output().await;
                    output
                }).await;

                match result {
                    Ok(Ok(output)) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let exit_code = output.status.code().unwrap_or(-1);

                        // 截断过长输出
                        let stdout_truncated = if stdout.len() > self.max_output_bytes {
                            format!("{}...(truncated)", &stdout[..self.max_output_bytes])
                        } else {
                            stdout.to_string()
                        };

                        yield ToolOutput::Result(serde_json::json!({
                            "exit_code": exit_code,
                            "stdout": stdout_truncated,
                            "stderr": stderr.to_string(),
                        }).to_string());
                    }
                    Ok(Err(e)) => {
                        yield ToolOutput::Result(serde_json::json!({
                            "exit_code": -1,
                            "stdout": "",
                            "stderr": format!("Failed to execute: {e}"),
                        }).to_string());
                    }
                    Err(_) => {
                        yield ToolOutput::Result(serde_json::json!({
                            "exit_code": -1,
                            "stdout": "",
                            "stderr": format!("Command timed out after {}s", timeout.as_secs()),
                        }).to_string());
                    }
                }
            }))
        }
    }
}
```

#### BashTool 安全模型

| 层级 | 防护机制 |
|------|---------|
| 命令级 | 白名单 / 黑名单过滤 |
| 时间 | 超时 kill |
| 输出 | 截断过长 stdout/stderr |
| 进程 | 子进程隔离，不继承父进程信号 |
| WASM | 在 wasm-guest 中不可用（无 std::process），编译时排除 |

---

### 3.2 FsTool——物理文件系统

对物理文件系统的读写操作。**仅 native 模式可用**。

```rust
/// File system operations: read, write, list, and search files.
///
/// All paths are resolved relative to the configured root directory.
/// Paths that escape the root (via ../) are rejected.
pub struct FsTool {
    /// 根目录——所有路径相对于此解析
    root: PathBuf,
    /// 允许写入（默认 false = 只读）
    writable: bool,
    /// 最大可读文件大小（默认 10MB）
    max_read_bytes: usize,
    /// 允许的文件扩展名（None = 不限制）
    allowed_extensions: Option<Vec<String>>,
}
```

#### Tool Schema

```json
{
  "name": "fs",
  "description": "Read, write, list, and search files in the workspace",
  "parameters": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": ["read", "write", "list", "search", "info"],
        "description": "The file system operation to perform"
      },
      "path": {
        "type": "string",
        "description": "Relative file or directory path"
      },
      "content": {
        "type": "string",
        "description": "Content to write (required for 'write' action)"
      },
      "pattern": {
        "type": "string",
        "description": "Search pattern (glob for 'search' action)"
      },
      "start_line": {
        "type": "integer",
        "description": "Start line for partial read (1-based, optional)"
      },
      "end_line": {
        "type": "integer",
        "description": "End line for partial read (1-based, inclusive, optional)"
      }
    },
    "required": ["action", "path"]
  }
}
```

#### 实现

```rust
impl FsTool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            writable: false,
            max_read_bytes: 10 * 1_048_576, // 10MB
            allowed_extensions: None,
        }
    }

    pub fn writable(mut self) -> Self { self.writable = true; self }
    pub fn read_only(mut self) -> Self { self.writable = false; self }
    pub fn with_max_read(mut self, bytes: usize) -> Self { self.max_read_bytes = bytes; self }
    pub fn with_extensions(mut self, exts: Vec<String>) -> Self {
        self.allowed_extensions = Some(exts); self
    }

    /// 路径安全检查——不允许逃逸 root
    fn resolve_path(&self, relative: &str) -> Result<PathBuf, AgentError> {
        let resolved = self.root.join(relative).canonicalize()
            .map_err(|e| AgentError::ToolExecution {
                tool_name: "fs".into(),
                message: format!("Path resolution failed: {e}"),
            })?;
        if !resolved.starts_with(&self.root) {
            return Err(AgentError::ToolExecution {
                tool_name: "fs".into(),
                message: "Path escapes root directory".into(),
            });
        }
        // 扩展名检查
        if let Some(allowed) = &self.allowed_extensions {
            if let Some(ext) = resolved.extension().and_then(|e| e.to_str()) {
                if !allowed.iter().any(|a| a == ext) {
                    return Err(AgentError::ToolExecution {
                        tool_name: "fs".into(),
                        message: format!("Extension '.{ext}' not allowed"),
                    });
                }
            }
        }
        Ok(resolved)
    }
}

impl Tool for FsTool {
    fn name(&self) -> &str { "fs" }
    fn description(&self) -> &str {
        "Read, write, list, and search files in the workspace"
    }
    fn parameters_schema(&self) -> serde_json::Value { /* 见上方 */ }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let action = args["action"].as_str().unwrap_or("read");
            let path_str = args["path"].as_str().unwrap_or(".");

            Ok(ToolResult::Output(stream! {
                match action {
                    "read" => {
                        let path = self.resolve_path(path_str)?;
                        let metadata = tokio::fs::metadata(&path).await
                            .map_err(|e| AgentError::ToolExecution {
                                tool_name: "fs".into(),
                                message: format!("Cannot read: {e}"),
                            })?;
                        if metadata.len() as usize > self.max_read_bytes {
                            yield ToolOutput::Result(format!(
                                "File too large: {} bytes (max {})",
                                metadata.len(), self.max_read_bytes
                            ));
                            return;
                        }

                        let content = tokio::fs::read_to_string(&path).await
                            .map_err(|e| AgentError::ToolExecution {
                                tool_name: "fs".into(),
                                message: format!("Read failed: {e}"),
                            })?;

                        // 支持行范围读取
                        let start = args.get("start_line")
                            .and_then(|v| v.as_u64())
                            .map(|n| (n as usize).saturating_sub(1));
                        let end = args.get("end_line")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as usize);

                        let output = if let Some(start) = start {
                            let lines: Vec<&str> = content.lines().collect();
                            let end = end.unwrap_or(lines.len()).min(lines.len());
                            lines[start..end].join("\n")
                        } else {
                            content
                        };

                        yield ToolOutput::Result(output);
                    }

                    "write" => {
                        if !self.writable {
                            yield ToolOutput::Result("Error: filesystem is read-only".into());
                            return;
                        }
                        let path = self.resolve_path(path_str)?;
                        let content = args["content"].as_str().unwrap_or("");
                        // 确保父目录存在
                        if let Some(parent) = path.parent() {
                            tokio::fs::create_dir_all(parent).await.ok();
                        }
                        tokio::fs::write(&path, content).await
                            .map_err(|e| AgentError::ToolExecution {
                                tool_name: "fs".into(),
                                message: format!("Write failed: {e}"),
                            })?;
                        yield ToolOutput::Result(format!(
                            "Written {} bytes to {}", content.len(), path_str
                        ));
                    }

                    "list" => {
                        let path = self.resolve_path(path_str)?;
                        let mut entries = tokio::fs::read_dir(&path).await
                            .map_err(|e| AgentError::ToolExecution {
                                tool_name: "fs".into(),
                                message: format!("List failed: {e}"),
                            })?;
                        let mut items = Vec::new();
                        while let Some(entry) = entries.next_entry().await.unwrap_or(None) {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let is_dir = entry.file_type().await
                                .map(|t| t.is_dir()).unwrap_or(false);
                            items.push(if is_dir {
                                format!("{name}/")
                            } else {
                                name
                            });
                        }
                        items.sort();
                        yield ToolOutput::Result(items.join("\n"));
                    }

                    "search" => {
                        let path = self.resolve_path(path_str)?;
                        let pattern = args["pattern"].as_str().unwrap_or("*");
                        yield ToolOutput::Delta(format!("Searching for '{pattern}'...\n"));
                        // 递归 glob 搜索
                        let matches = glob_walk(&path, pattern).await;
                        yield ToolOutput::Result(matches.join("\n"));
                    }

                    "info" => {
                        let path = self.resolve_path(path_str)?;
                        let meta = tokio::fs::metadata(&path).await
                            .map_err(|e| AgentError::ToolExecution {
                                tool_name: "fs".into(),
                                message: format!("Info failed: {e}"),
                            })?;
                        yield ToolOutput::Result(serde_json::json!({
                            "path": path_str,
                            "size": meta.len(),
                            "is_dir": meta.is_dir(),
                            "is_file": meta.is_file(),
                            "readonly": meta.permissions().readonly(),
                        }).to_string());
                    }

                    other => {
                        yield ToolOutput::Result(format!("Unknown action: {other}"));
                    }
                }
            }))
        }
    }
}
```

#### FsTool 安全模型

| 层级 | 防护机制 |
|------|---------|
| 路径 | root jail——canonicalize + starts_with 禁止逃逸 |
| 读写 | 默认只读，需 `.writable()` 显式开启 |
| 大小 | 读取上限 max_read_bytes |
| 类型 | 可限制允许的文件扩展名 |
| WASM | wasm-guest 中不可用（无 tokio::fs），使用 VirtualFsTool 代替 |

---

### 3.3 VirtualFsTool——虚拟文件系统（WASM 兼容）

沙箱化的内存文件系统，适用于 WASM Guest 或需要隔离的场景。

```rust
/// Virtual in-memory filesystem — WASM compatible, fully sandboxed.
///
/// All files exist only in memory. Useful for:
/// - WASM guest modules (no real filesystem access)
/// - Sandboxed code generation (write code, read back)
/// - Testing (deterministic, no side effects)
pub struct VirtualFsTool {
    fs: RefCell<VirtualFs>,
}

/// 内存文件系统数据结构
struct VirtualFs {
    files: HashMap<String, String>,        // path → content
    max_files: usize,                       // 文件数上限（默认 1000）
    max_file_size: usize,                   // 单文件大小上限（默认 1MB）
    max_total_size: usize,                  // 总大小上限（默认 50MB）
    current_total_size: usize,
}
```

#### 实现

```rust
impl VirtualFsTool {
    pub fn new() -> Self {
        Self {
            fs: RefCell::new(VirtualFs {
                files: HashMap::new(),
                max_files: 1000,
                max_file_size: 1_048_576,
                max_total_size: 50 * 1_048_576,
                current_total_size: 0,
            }),
        }
    }

    pub fn with_max_files(self, n: usize) -> Self { ... }
    pub fn with_max_file_size(self, bytes: usize) -> Self { ... }

    /// 预装文件（用于初始化沙箱环境）
    pub fn with_file(self, path: impl Into<String>, content: impl Into<String>) -> Self {
        let path = path.into();
        let content = content.into();
        self.fs.borrow_mut().files.insert(path, content);
        self
    }
}

impl Tool for VirtualFsTool {
    fn name(&self) -> &str { "vfs" }
    fn description(&self) -> &str {
        "Virtual filesystem: read, write, list files in a sandboxed in-memory filesystem"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        // 与 FsTool 相同的 action/path/content schema
        // 但不支持 search（无 glob 库要求）
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "list", "delete"],
                    "description": "The virtual filesystem operation"
                },
                "path": {
                    "type": "string",
                    "description": "Virtual file path"
                },
                "content": {
                    "type": "string",
                    "description": "Content for write action"
                }
            },
            "required": ["action", "path"]
        })
    }

    fn execute(&self, args: serde_json::Value)
        -> impl Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        async move {
            let action = args["action"].as_str().unwrap_or("read");
            let path = args["path"].as_str().unwrap_or("").to_string();

            Ok(ToolResult::Output(stream! {
                match action {
                    "read" => {
                        let fs = self.fs.borrow();
                        match fs.files.get(&path) {
                            Some(content) => yield ToolOutput::Result(content.clone()),
                            None => yield ToolOutput::Result(
                                format!("Error: file not found: {path}")
                            ),
                        }
                    }
                    "write" => {
                        let content = args["content"].as_str().unwrap_or("").to_string();
                        let mut fs = self.fs.borrow_mut();
                        if fs.files.len() >= fs.max_files && !fs.files.contains_key(&path) {
                            yield ToolOutput::Result("Error: max files limit reached".into());
                            return;
                        }
                        if content.len() > fs.max_file_size {
                            yield ToolOutput::Result("Error: file too large".into());
                            return;
                        }
                        // 更新总大小
                        let old_size = fs.files.get(&path).map(|c| c.len()).unwrap_or(0);
                        let new_total = fs.current_total_size - old_size + content.len();
                        if new_total > fs.max_total_size {
                            yield ToolOutput::Result("Error: total storage limit exceeded".into());
                            return;
                        }
                        fs.current_total_size = new_total;
                        fs.files.insert(path.clone(), content.clone());
                        yield ToolOutput::Result(
                            format!("Written {} bytes to {path}", content.len())
                        );
                    }
                    "list" => {
                        let fs = self.fs.borrow();
                        let prefix = if path == "." || path.is_empty() {
                            String::new()
                        } else {
                            format!("{}/", path.trim_end_matches('/'))
                        };
                        let mut entries: Vec<String> = fs.files.keys()
                            .filter(|k| k.starts_with(&prefix) || prefix.is_empty())
                            .cloned()
                            .collect();
                        entries.sort();
                        yield ToolOutput::Result(entries.join("\n"));
                    }
                    "delete" => {
                        let mut fs = self.fs.borrow_mut();
                        match fs.files.remove(&path) {
                            Some(content) => {
                                fs.current_total_size -= content.len();
                                yield ToolOutput::Result(format!("Deleted: {path}"));
                            }
                            None => yield ToolOutput::Result(
                                format!("Error: file not found: {path}")
                            ),
                        }
                    }
                    other => {
                        yield ToolOutput::Result(format!("Unknown action: {other}"));
                    }
                }
            }))
        }
    }
}
```

---

### 3.4 内置 Tool 总览

| Tool | Feature Flag | 平台 | 描述 |
|------|-------------|------|------|
| `BashTool` | `tool-bash` | native only | Shell 命令执行，白/黑名单，超时 |
| `FsTool` | `tool-fs` | native only | 物理文件系统读写，路径沙箱 |
| `VirtualFsTool` | `tool-fs-virtual` | native + WASM | 内存虚拟文件系统，完全沙箱 |

### 3.5 使用示例

```rust
use remi_agentloop::prelude::*;
use remi_agentloop::tools::{BashTool, FsTool, VirtualFsTool};

// Native——编程 Agent
let agent = AgentBuilder::new()
    .model(model)
    .system("You are a coding assistant. Use bash and fs tools to help the user.")
    .tool(BashTool::new()
        .with_timeout(Duration::from_secs(60))
        .with_working_dir("/home/user/project"))
    .tool(FsTool::new("/home/user/project").writable())
    .build();

// WASM Guest——沙箱内执行
let agent = AgentBuilder::new()
    .model(model)
    .system("You are a code generator.")
    .tool(VirtualFsTool::new()
        .with_file("template.rs", include_str!("template.rs")))
    .build();

// 混合——宏定义 tool + 内置 tool
#[tool]
async fn analyze_code(
    /// Path to the source file
    path: String,
    ctx: &ToolContext,
) -> Result<String, AgentError> {
    // 自定义逻辑
    Ok("analysis complete".into())
}

let agent = AgentBuilder::new()
    .model(model)
    .tool(BashTool::new())
    .tool(FsTool::new(".").read_only())
    .tool(AnalyzeCode)  // 宏生成的 tool
    .build();
```

---

## 4. 模块结构更新

```
src/
├── tool/
│   ├── mod.rs          # Tool trait, ToolDefinition, ToolContext
│   ├── registry.rs     # ToolRegistry, DynTool
│   ├── bash.rs         # BashTool          [feature: tool-bash]
│   ├── fs.rs           # FsTool            [feature: tool-fs]
│   └── vfs.rs          # VirtualFsTool     [feature: tool-fs-virtual]
├── ...
macros/
├── Cargo.toml          # proc-macro crate
└── src/
    └── lib.rs          # #[tool] 宏实现
```

### Feature Flags 更新

```toml
[features]
default = ["native"]
native = ["dep:tokio"]

# 内置 Tool
tool-bash = ["native"]                          # 依赖 tokio::process
tool-fs = ["native"]                            # 依赖 tokio::fs
tool-fs-virtual = []                            # 纯内存，WASM 兼容
tools = ["tool-bash", "tool-fs", "tool-fs-virtual"]

# 传输层
http-client = ["dep:reqwest"]
http-server = ["dep:axum"]
wasm-host = ["dep:wasmi"]
wasm-guest = ["dep:wasm-bindgen", "dep:wasm-bindgen-futures"]

# 追踪
tracing-langsmith = ["dep:reqwest", "dep:chrono"]
```

---

## 5. Roadmap 影响

Phase 3 新增：

- `9a.` `macros/` — `#[tool]` 过程宏
- `9b.` `tool/bash.rs` — BashTool [tool-bash]
- `9c.` `tool/fs.rs` — FsTool [tool-fs]
- `9d.` `tool/vfs.rs` — VirtualFsTool [tool-fs-virtual]

Phase 6 新增：

- `32.` `#[tool]` 宏 + 内置 tool 端到端示例
- `33.` BashTool 安全策略测试（白/黑名单 + 超时）
