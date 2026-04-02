# Sub-Agent / Sub-Session

> 如何把独立专职 agent 暴露成一个 tool，并把它的执行过程流式投影到子会话里

## 目标

这个模式适合下面这类需求：

- 主 agent 保持通用协调角色
- 某些任务交给专门 agent 处理，例如计算、检索、规划、代码修复
- 子 agent 的中间执行过程需要可观察
- 但主 agent 上下文里只应该接收最终结论，不应混入子 agent 的所有中间 delta

在 `remi-agentloop` 里，这个能力由三部分组成：

1. `ToolOutput::SubSession(...)`
2. `AgentEvent::SubSession(...)`
3. `ProtocolEvent::SubSession`

以及一个框架 helper：

- `remi_agentloop_deepagent::SubAgentToolAdapter`

## 心智模型

不要把 sub agent 想成“主 agent 的隐藏递归调用”。

更准确的理解是：

- 子 agent 是一个普通工具背后的执行器
- 主 agent 调用工具
- 工具内部启动子 agent
- 子 agent 的中间事件被投影成 `SubSession` 事件流
- 工具在最后再返回一个普通 `ToolResult`

这样有两个好处：

1. 父会话保持干净，主模型只消费最终工具结果
2. UI 或外部观察者仍然能完整看到子 agent 的执行细节

## 事件流

标准事件流如下：

```text
Parent model
  -> ToolCallStart(calculator_subagent)
  -> ToolCallDelta(...args...)

SubAgentToolAdapter
  -> ToolOutput::SubSession(Start)
  -> ToolOutput::SubSession(Delta/Thinking/ToolCall/...)
  -> ToolOutput::SubSession(Done)
  -> ToolOutput::Result(final answer)

AgentLoop
  -> AgentEvent::SubSession(...)
  -> AgentEvent::ToolResult(...final answer...)

Protocol
  -> ProtocolEvent::SubSession(...)
  -> ProtocolEvent::ToolResult(...)
```

注意这个约束：

- 子 agent 的 `Delta` 不会自动变成父 agent 的 `TextDelta`
- 父 agent 真正收到的是最后一个工具结果

## 核心类型

### `SubSessionEvent`

`SubSessionEvent` 描述一个归属于父 tool call 的子会话事件，核心字段包括：

- `parent_tool_call_id`
- `sub_thread_id`
- `sub_run_id`
- `agent_name`
- `title`
- `depth`
- `payload`

其中 `payload` 是 `SubSessionEventPayload`，目前支持：

- `Start`
- `Delta`
- `ThinkingStart`
- `ThinkingEnd`
- `ToolCallStart`
- `ToolCallArgumentsDelta`
- `ToolDelta`
- `ToolResult`
- `TurnStart`
- `Done`
- `Error`

### `ToolOutput::SubSession`

这是工具层和父 loop 之间的桥。

只要你的工具内部运行了一个子 agent，就应该把中间事件转换成这个类型，而不是把所有细节拼成一段 markdown 文本。

### `ProtocolEvent::SubSession`

这是跨进程、跨网络、跨 WASM 的标准线上格式。只要 consumer 认这个事件，sub session 就可以被传输和重建。

## 最小使用方式

最简单的方式是直接用 `SubAgentToolAdapter`。

```rust
use async_stream::stream;
use remi_agentloop::prelude::*;
use remi_agentloop_deepagent::{SubAgentEventStream, SubAgentToolAdapter};
use serde_json::json;

fn research_specialist_tool<M>(model: M) -> SubAgentToolAdapter
where
    M: ChatModel + Clone + Send + Sync + 'static,
{
    SubAgentToolAdapter::new(
        "research_specialist",
        "Delegate research to a dedicated sub-agent.",
        json!({
            "type": "object",
            "properties": {
                "topic": { "type": "string" }
            },
            "required": ["topic"]
        }),
        "research",
        |arguments| arguments.get("topic").and_then(|v| v.as_str()).map(ToString::to_string),
        move |arguments| {
            let model = model.clone();
            Box::pin(async move {
                let topic = arguments
                    .get("topic")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AgentError::tool("research_specialist", "missing topic"))?
                    .to_string();

                let agent = AgentBuilder::new()
                    .model(model)
                    .system("You are a focused research specialist. Return only the final summary.")
                    .max_turns(6)
                    .build_loop();

                Ok(Box::pin(stream! {
                    match agent.chat(LoopInput::start(topic)).await {
                        Ok(inner_stream) => {
                            let mut inner_stream = std::pin::pin!(inner_stream);
                            while let Some(event) = inner_stream.next().await {
                                yield event;
                            }
                        }
                        Err(error) => {
                            yield AgentEvent::Error(error);
                        }
                    }
                }) as SubAgentEventStream)
            })
        },
    )
}
```

这个 helper 已经帮你做了这些事：

- 监听子 agent 的 `RunStart`
- 转发子 agent 的文本、thinking、tool call、tool result、turn 事件
- 在结束时发出 `SubSessionEventPayload::Done`
- 把最终文本作为普通 tool result 返回给父 loop

## 手写实现时的规则

如果你不想用 `SubAgentToolAdapter`，自己手写也可以，但要遵守这些规则：

1. 子 agent 开始时，尽早发出 `SubSession(Start)`
2. 子 agent 的中间文本不要直接混到父 agent `TextDelta`
3. 子 agent 内部如果又调用工具，继续发 `SubSession(ToolCall*)`
4. 正常结束时先发 `SubSession(Done)`，再返回工具最终结果
5. 异常结束时发 `SubSession(Error)`，并决定是否把错误文本作为工具结果返回给父 agent

## `parent_tool_call_id` 是怎么绑定的

通常工具实现不需要自己知道父 tool call id。

`AgentLoop` 在执行工具时，如果发现 `ToolOutput::SubSession` 里的 `parent_tool_call_id` 为空，会自动把当前工具调用 ID 填进去。

这就是为什么子 agent helper 可以只负责“产生子会话事件”，而不用显式感知父 loop 细节。

## UI / SDK 侧应该怎么处理

框架层只负责标准化事件，不负责 UI。

消费方通常应该：

1. 按 `parent_tool_call_id` 聚合 sub session
2. 把它附着到对应父 tool-call 节点下
3. 把 `Delta`、`Thinking`、`ToolCall`、`ToolResult` 渲染成嵌套项
4. 把 `Done.final_output` 当成子会话摘要或结束状态

是否展示成可折叠卡片、时间线、树结构，属于产品层决定。

## 何时适合用 sub agent

适合：

- 明确的专职能力边界，例如 calculator / planner / retriever / coder
- 子任务可能多轮运行
- 希望保留中间过程用于调试、可视化、回放
- 不希望污染父 agent 历史

不适合：

- 只是一个一步到位的普通工具
- 中间过程没有观察价值
- 最终只需要一个同步函数调用

## 当前框架边界

已经在框架层通用化的部分：

- `SubSessionEvent`
- `SubSessionEventPayload`
- `ToolOutput::SubSession`
- `ProtocolEvent::SubSession`
- `SubAgentToolAdapter`

仍然属于产品层的部分：

- UI 结构
- SDK 本地缓存形态
- 产品自定义 protobuf / HTTP surface
- 专职 agent 的 prompt 与暴露策略

## 并行与异步扩展

这套设计在框架层面是可以向“并行子 agent”和“异步 task 模式”扩展的，但当前产品实现还没有全部补齐。

### 1. 多个 sub agent 并行执行

框架层面：

- 可以支持。
- `SubSessionEvent` 自带 `parent_tool_call_id`、`sub_thread_id`、`sub_run_id`。
- 这意味着即使多个子会话事件在流里交错出现，consumer 也能按身份把它们拆开重建。
- `AgentLoop` 本身也已经有并行执行多个 tool call 的能力，所以如果同一轮里模型发出多个 sub-agent tool call，它们可以并发运行。

当前限制主要在消费层：

- 现有 `remi-client-sdk` 是按 `parent_tool_call_id` 维护单个 `ProtocolSubSessionDraft`。
- 当前 `CachedMessage` 只有一个 `sub_session` 字段，不是 `Vec<CachedSubSession>`。
- 这意味着“一个父 tool call 下挂多个并行子会话”在产品缓存/UI里还没有完整表达。

所以结论是：

- 多个不同父 tool call 的 sub-agent 并行：框架上天然可表达。
- 同一个父 tool call 内再分叉多个并行 sub-session：框架事件模型可表达，但当前 SDK/UI 还需要改成“按 `(parent_tool_call_id, sub_session_id)` 聚合，并允许一个父节点下挂多个 sub session”。

### 2. 异步执行，返回 task id，再轮询结果

框架层面：

- 也可以支持，但不属于当前同步 `SubAgentToolAdapter` 的默认语义。
- 当前 adapter 的语义是“当前 tool 调用内启动子 agent，流式转发过程，结束时返回最终结果”。

如果改成异步 task 模式，推荐的设计边界是：

1. `launch_sub_agent_task(...)` 启动后台子任务，立即返回 `task_id`
2. 主 agent 可以调用 `poll_sub_agent_task(task_id)` 查询状态
3. 任务完成后再把最终结果作为普通 tool result 返回
4. 子任务自己的中间事件仍然可以继续写成 sub-session 事件流，供 UI/trace 观察

这要求一个额外的外层组件：

- 任务存储 / task registry
- 后台执行器
- 轮询或回调机制

也就是说，真正的“异步子 agent”不是 core loop 自己就能单独完成的，它需要 outer orchestration layer 配合。

当前框架已经具备的基础能力：

- `NeedToolExecution`
- `LoopInput::Resume`
- 可序列化 `AgentState`
- checkpoint / state recovery

这些已经足够承载异步任务编排，但如果要把它做成明确的一等模式，后续最好再补两类框架约定：

- task 生命周期抽象，例如 `queued / running / completed / failed`
- 子会话与 task 的绑定字段，例如稳定 `task_id`

### 推荐的未来兼容方向

如果现在就希望后续演进平滑，建议保持下面几个约束：

1. 不要把 sub session 绑定死成“每个父 tool call 只能有一个子会话”
2. UI/SDK 聚合键优先使用 `sub_session_id`，而不是只用 `parent_tool_call_id`
3. 把“同步完成返回结果”和“异步返回 task_id”视为两种 tool contract，而不是硬塞进同一个 adapter 默认行为
4. 后台 task 模式优先由外层 orchestration 负责，core 负责标准事件和 resume 能力

## 建议

如果你要新增一个 specialist agent，优先走这条路径：

1. 先把 specialist 做成一个独立 tool
2. 用 `SubAgentToolAdapter` 承接它的运行
3. 只把最终结果返回给父 agent
4. 让消费方按 `SubSession` 做可观察性展示

这样最接近框架现有能力，也最容易复用到 remote、local-WASM、以及任意上层产品协议。