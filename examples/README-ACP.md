# ACP (Agent Communication Protocol) 示例

## 概述

ACP 是一个完整的多智能体通信协议，支持智能体发现、能力匹配、任务委派和流式通信。

## 示例列表

### 1. acp-multi-agent - 多智能体本地示例

演示多个专门智能体的协作：
- SearchAgent - 搜索专家
- CodeAgent - 代码分析专家
- OrchestratorAgent - 编排器（委派任务给其他智能体）

```bash
cd acp-multi-agent
cargo run
```

### 2. acp-http-server - HTTP 服务器

通过 HTTP/SSE 暴露 ACP 智能体服务：

```bash
cd acp-http-server
cargo run
```

服务器启动后访问：
- `POST http://localhost:8080/acp` - 执行请求

测试命令：
```bash
curl -X POST http://localhost:8080/acp \
  -H "Content-Type: application/json" \
  -d '{"content":"calculate 10 + 20"}'
```

### 3. acp-http-client - HTTP 客户端

连接到 ACP 服务器并执行请求：

```bash
# 先启动服务器
cd acp-http-server && cargo run &

# 在另一个终端运行客户端
cd acp-http-client
cargo run
```

## 快速开始

### 定义智能体

```rust
use remi_agentloop_transport::acp::*;

struct MyAgent {
    agent_id: AgentId,
}

impl AcpAgent for MyAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: AgentId::new("my_agent"),
            name: "My Agent".into(),
            description: "Does something useful".into(),
            tools: vec![...],
            domains: vec!["domain1".into()],
            ...
        }
    }

    fn execute(&self, request: AcpRequest) 
        -> Pin<Box<dyn Future<Output = ...>>>
    {
        Box::pin(async move {
            Ok(Box::pin(stream! {
                yield AcpEvent::AgentStart { ... };
                yield AcpEvent::ContentDelta { delta: "Hello!".into() };
                yield AcpEvent::AgentEnd { ... };
            }))
        })
    }
}
```

### 创建路由器

```rust
let registry = AgentRegistry::new();
let router = AcpRouter::new(registry)
    .register_agent(Box::new(MyAgent::new()));

// 执行请求
let request = AcpRequest {
    content: AcpContent::text("do something"),
    ...
};
let mut stream = router.execute(request).await?;
```

### 启动 HTTP 服务器

```rust
let server = AcpServer::new(move |req| {
    let router = router.clone();
    async move { router.execute(req).await }
})
.bind(([0, 0, 0, 0], 8080))
.serve()
.await?;
```

## 更多文档

- [ACP 协议设计文档](../docs/acp-protocol.md)
- [实现总结](../docs/acp-implementation-summary.md)
