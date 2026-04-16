# 如何实现一个 Markdown 驱动的动态 Agent 系统

> 目标场景：应用层希望把 agent 和 subagent 都定义在 markdown 文件里，在运行时动态分配 name、system prompt、model、tool、subagent 关系，而不是把这些内容全部写死在 Rust 代码里。

## 1. 结论先行

**现在已经可以做，而且大部分关键能力已经在当前框架里具备。**

当前能力判断如下：

- **动态 name**：可以做，直接体现在 `Message.name`
- **动态 system prompt**：可以做，最实用的方法是把它编码成 `history` 中的 system message
- **动态 model / temperature / max_tokens**：可以做，`LoopInput::Start` 已支持
- **动态 tool 集合**：可以做，靠 `ToolRegistry` / `extra_tools` / `ToolLayer`
- **动态 subagent**：可以做，建议通过 catalog-driven dispatcher tool 实现
- **动态 tracing / cancellation 传播**：可以做，靠 `ChatCtx`

所以真正的问题不是“框架支不支持”，而是“应用层怎么组织 spec、catalog、tool factory 和运行时装配逻辑”。

---

## 2. 推荐的总体架构

推荐把运行时分成四层：

1. **Markdown Spec 层**：每个 agent 一个 markdown 文件
2. **Catalog 层**：把所有 markdown 解析成统一 `AgentSpec`
3. **Runtime Assembly 层**：根据 spec 动态拼装 `Message`、`ToolRegistry`、`ToolLayer`
4. **Dispatcher / Subagent 层**：通过统一工具把一个 agent 调另一个 agent

结构示意：

```text
markdown files
    -> AgentSpec
    -> AgentCatalog
    -> RuntimeAgentFactory
    -> Built agent / layered agent
    -> catalog-driven subagent dispatcher
```

不要把 markdown 直接解释成“可执行代码”。

正确做法是：

- markdown 只描述 agent 配置和 prompt 内容
- tool 的真实实现仍由应用层注册到一个受控 catalog 里
- markdown 里只引用工具名，不直接嵌入任意执行逻辑

这点很重要，否则系统会很快变成不可控的脚本执行器。

---

## 3. Markdown 文件应该长什么样

推荐用：

- YAML frontmatter 放结构化配置
- markdown body 放 system prompt 主体、附加规则、few-shot 模板等

例如：

```md
---
id: planner
name: Planner
model: gpt-4.1
temperature: 0.2
max_tokens: 4000
tools:
  - todo__add
  - todo__list
  - task__run
subagents:
  - researcher
  - coder
metadata:
  team: core
---

# Role

You are the planning agent.

# Rules

1. Break work into steps.
2. Delegate factual lookup to researcher.
3. Delegate implementation to coder.

# Output Contract

- Always produce a plan before execution.
```

建议 frontmatter 里只保留“机器消费字段”：

- `id`
- `name`
- `model`
- `temperature`
- `max_tokens`
- `tools`
- `subagents`
- `metadata`

正文 markdown 则作为 system prompt 模板主体。

---

## 4. 先定义统一的运行时数据结构

建议在应用层定义类似这样的结构：

```rust
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub system_prompt_markdown: String,
    pub model: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub tool_names: Vec<String>,
    pub subagent_ids: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}
```

以及：

```rust
pub struct AgentCatalog {
    specs: HashMap<String, AgentSpec>,
}
```

Catalog 的职责应该很纯粹：

- load / reload markdown
- parse frontmatter
- validate references
- 暴露 `get(spec_id)`

不要让 catalog 自己直接执行 agent。执行应交给独立 runtime 层。

---

## 5. Tool 不要直接从 markdown 构造，应该从应用层 tool catalog 构造

这是动态 agent 系统最重要的一条工程原则。

### 5.1 不推荐

不要让 markdown 直接描述：

- 一段 Rust 代码
- 一段 shell 脚本
- 一段任意函数体

### 5.2 推荐

应用层先维护一个受控的 `ToolFactoryCatalog`：

```rust
pub trait ToolFactory: Send + Sync {
    fn build(&self, agent_spec: &AgentSpec, catalog: &AgentCatalog) -> Box<dyn DynToolLike>;
}

pub struct ToolFactoryCatalog {
    inner: HashMap<String, Arc<dyn ToolFactory>>,
}
```

然后 markdown 只引用工具名：

```yaml
tools:
  - bash
  - fs_read
  - task__run
```

运行时再把这些名字解析成真正的 tool 实例。

这样做的好处：

- 能做权限控制
- 能做不同 agent 的 tool 白名单
- 能做带 agent-spec 上下文的实例化
- 不会把 markdown 变成任意代码执行入口

---

## 6. 动态 name、system prompt、model、tools 分别怎么落地

### 6.1 动态 name

最简单的做法是把发起消息构造成：

```rust
let input = LoopInput::start_message(
    Message::user(user_text).with_name(spec.name.clone())
);
```

如果你希望“agent 名字”体现在 assistant/tool 侧，也可以把它放进：

- metadata
- tracing span scope key
- protocol side custom events

### 6.2 动态 system prompt

当前最实用的做法，不是等一个新字段，而是直接把 system prompt 注入到 `history` 里：

```rust
let history = vec![Message::system(render_system_prompt(&spec))];

let input = LoopInput::start_message(Message::user(task))
    .history(history);
```

这在当前框架里是可行的，而且语义清晰。

如果后续 core 再补一个 `LoopInput::Start.system_prompt`，那只是 ergonomics 提升，不是能力补洞。

### 6.3 动态 model / temperature / max_tokens

直接用 `LoopInput::Start` 的 builder：

```rust
let input = LoopInput::start_message(Message::user(task))
    .history(vec![Message::system(render_system_prompt(&spec))])
    .model(spec.model.clone().unwrap_or(default_model))
    .temperature(spec.temperature.unwrap_or(0.2))
    .max_tokens(spec.max_tokens.unwrap_or(4096));
```

### 6.4 动态 tool 集合

当前推荐分成两类：

- **暴露给模型看的工具定义**：放进 `extra_tools`
- **真正由外层拥有并自动处理的工具**：用 `ToolLayer`

这两种都能动态生成，但职责不同。

---

## 7. `ToolLayer` 应该怎么用于动态 agent

动态 agent 系统里，不应该只构造一个大而全 registry。更好的做法是**按职责叠 layer**。

例如：

1. 平台级基础工具层
2. 业务域工具层
3. 当前 spec 专属工具层
4. subagent dispatcher 工具层

示意：

```rust
let base = DefaultToolRegistry::new()
    .tool(TimeTool)
    .tool(HttpFetchTool)
    .into_layer();

let business = build_business_registry(spec).into_layer();
let subagents = build_subagent_registry(spec, catalog).into_layer();

let agent = AgentBuilder::new()
    .model(shared_model.clone())
    .build_loop()
    .layer(base)
    .layer(business)
    .layer(subagents);
```

这样做的价值在于：

- 每层都可以单独启停
- 每层都可以挂不同 hook / tracing / policy
- spec 变化时，不用重写所有工具装配逻辑

---

## 8. subagent 最推荐的实现方式：catalog-driven dispatcher tool

不要为每个子 agent 都手写一个 `FooSubagentTool`。推荐提供一个通用 tool：

```text
agent__run(agent_id, task, overrides?)
```

模型只需要知道：

- 它可以调用 `agent__run`
- `agent_id` 必须来自允许列表

### 8.1 dispatcher tool 的职责

- 校验 `agent_id` 是否存在
- 校验当前 agent 是否允许调用该 subagent
- 从 catalog 取出子 `AgentSpec`
- 用 `ctx.fork()` 派生子上下文
- 按子 spec 动态组装子 agent
- 运行子 agent
- 把子 agent 的结构化事件作为 `ToolOutput::Custom("subagent_event", ...)` 转发
- 最终把子 agent 文本结果回填为本次 tool result

这和当前 `SubAgentTaskTool` 的方向是一致的，只是把“静态预配置的某个子 agent”提升成“运行时从 catalog 解析目标 agent”。

### 8.2 为什么推荐通用 dispatcher，而不是每个 subagent 一个 tool 名

因为当 subagent 真正动态化后：

- tool 数量会随 markdown 文件数线性爆炸
- prompt 里会出现大量静态工具定义冗余
- catalog reload 后 tool surface 也跟着频繁变化

统一 dispatcher 更稳定：

- 对模型是一个固定工具面
- 对应用层是一个受控调度口
- 对 tracing/cancellation/resume 是统一路径

---

## 9. 一个推荐的运行时执行流程

可以按下面的顺序组织：

### 步骤 1：启动时加载全部 markdown

```text
scan directory -> parse markdown -> build AgentCatalog -> validate references
```

校验至少包括：

- `tools` 是否都能在 tool factory catalog 中解析
- `subagents` 是否都存在
- 是否有循环引用策略约束

### 步骤 2：收到请求时选择目标 spec

例如：

- 路由层决定本次使用 `planner`
- 或者用户请求里显式指定 `agent_id`

### 步骤 3：根据 spec 动态组装 runtime agent

组装内容包括：

- system prompt history
- request-level model overrides
- 当前 spec 可用 tools
- 当前 spec 可调用 subagents
- 当前 spec metadata

### 步骤 4：创建输入

```rust
let input = LoopInput::start_message(Message::user(task))
    .history(vec![Message::system(render_system_prompt(spec))])
    .metadata(spec.metadata.clone().unwrap_or(Value::Null))
    .extra_tools(dynamic_defs_for_model_view);
```

### 步骤 5：运行，并通过 `ChatCtx` 维持链路

父 agent：

- 创建 root `ChatCtx`

子 agent：

- `let child_ctx = ctx.fork();`

这样 tracing lineage 和 cancellation 都会天然延续。

---

## 10. 一个最小实现骨架

下面这个骨架足够作为应用层起点：

```rust
pub struct DynamicAgentRuntime<M> {
    model: M,
    catalog: Arc<AgentCatalog>,
    tool_factories: Arc<ToolFactoryCatalog>,
}

impl<M> DynamicAgentRuntime<M>
where
    M: ChatModel + Clone + Send + Sync + 'static,
{
    pub async fn run(
        &self,
        ctx: ChatCtx,
        agent_id: &str,
        user_message: Message,
    ) -> Result<impl Stream<Item = AgentEvent>, AgentError> {
        let spec = self.catalog.get(agent_id).unwrap().clone();

        let layered_agent = self.build_agent_for_spec(&spec);
        let input = LoopInput::start_message(user_message)
            .history(vec![Message::system(spec.system_prompt_markdown.clone())])
            .metadata(spec.metadata.clone().unwrap_or(serde_json::Value::Null));

        layered_agent.chat(ctx, input).await
    }

    fn build_agent_for_spec(&self, spec: &AgentSpec) -> impl Agent<Request = LoopInput, Response = AgentEvent, Error = AgentError> {
        let base = AgentBuilder::new()
            .model(self.model.clone())
            .build_loop();

        let dynamic_registry = self.build_registry_for_spec(spec);

        base.layer(dynamic_registry.into_layer())
    }
}
```

这里最重要的不是具体语法，而是职责划分：

- catalog 负责“有什么 agent”
- runtime 负责“本次怎么装配它”
- tool factory 负责“工具怎么实例化”

---

## 11. 动态 agent 系统里最容易踩的坑

### 11.1 把 markdown 直接当执行脚本

这是最危险的一种实现。正确做法是：

- markdown 只配置
- 真正执行能力来自应用层已注册工具

### 11.2 每个 agent 都 build 一整套互相独立的大对象

如果模型 client、transport、tracer、tool factory 都重复构造，运行时开销会很差。

正确做法：

- 共享 model client
- 共享 tool factory catalog
- 共享 tracer / store
- 每次请求只做轻量 runtime assembly

### 11.3 把 subagent 写成静态硬编码分支

如果代码里到处都是：

```rust
if agent_id == "planner" { ... }
if agent_id == "coder" { ... }
```

那这个系统本质上还是静态的。

应该让：

- spec lookup
- tool binding
- subagent permission

都走 catalog 数据驱动。

### 11.4 忽略循环调用与深度限制

动态 subagent 一定要加保护：

- 最大深度
- 最大调用次数
- 禁止 self-call 或同环递归
- 可选的预算控制（token / turns / wall time）

这类约束可以放进 metadata 或 `ChatCtx.user_state` 里统一维护。

---

## 12. 当前框架下最推荐的落地策略

如果你现在就要在这个仓库之上做动态 agent，建议按下面路线走：

1. **先做 markdown -> `AgentSpec` -> `AgentCatalog`**
2. **把现有工具实现统一放进 `ToolFactoryCatalog`**
3. **先用 `Message::system(...) + history` 解决动态 system prompt**
4. **用 `ToolRegistry::into_layer()` 叠出 spec 专属工具层**
5. **用一个通用 `agent__run` dispatcher tool 实现 subagent**
6. **所有父子调用都通过 `ChatCtx::fork()` 传递上下文**

这条路线与当前框架的自然形状是对齐的，不需要逆着 API 硬拧。

---

## 13. 如果要让这个方向更顺手，建议补的三个小能力

当前已经能做，但从工程 ergonomics 上看，下面三点仍值得补：

### 13.1 `LoopInput::Start.system_prompt: Option<String>`

现在可用 `history[0] = Message::system(...)` 解决，但显式字段会更直观。

### 13.2 一个更公共的动态 subagent runner 构造入口

比如允许直接传：

- catalog
- target agent id resolver
- runtime overrides

而不是主要依赖静态风格的 `SubAgentTaskTool::new(model, prompt, turns)`。

### 13.3 一个标准化的 spec validation/report API

动态 agent 系统上线后，错误更多会发生在：

- markdown 配置错
- tool 名写错
- subagent 引用错

所以最好有统一 validation 报告，而不是在第一次运行时才报错。

---

## 14. 一句话总结

当前这套框架已经足以支持“markdown 定义 agent，运行时动态装配 name / system prompt / model / tool / subagent”的系统。

最合理的实现方式不是把 markdown 变成代码，而是：

- **markdown 产出 `AgentSpec`**
- **catalog 管理 spec**
- **tool factory 负责受控实例化**
- **`ToolLayer` 负责运行时拼装工具面**
- **`ChatCtx` 负责把 tracing / cancellation / nested lineage 贯穿到父子 agent 全链路**

按这个方向做，系统会既动态，又还保持工程可控。