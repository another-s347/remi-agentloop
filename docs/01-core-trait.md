# 核心 Trait 设计

> Agent trait、AgentExt 扩展方法、Layer 组合模式、BoxedAgent 动态分发

## Agent Trait

使用 Rust 1.75+ 的 RPITIT（Return Position Impl Trait In Trait），核心抽象为：

```rust
/// 核心抽象：异步流式，全泛型，无 Send bound
pub trait Agent {
    type Request;
    type Response;
    type Error;

    fn chat(
        &self,
        req: Self::Request,
    ) -> impl Future<Output = Result<impl Stream<Item = Self::Response>, Self::Error>>;
}
```

### 设计要点

- **全泛型**：`Request`、`Response`、`Error` 均为关联类型，调用者在编译期完全可见
- **无 Send bound**：trait 层面不约束线程安全，由具体实现类型的 Send-ness 决定。WASM（单线程）天然满足，native 下由实现者的具体类型保证，调用侧（如 `tokio::spawn`）按需触发约束检查
- **RPITIT 返回**：实现者可直接写 `async fn` 或 `async { stream! { ... } }`，编译器推导具体类型链，零 vtable 开销

### Trade-off

- **不 object-safe**：无法 `dyn Agent`。需要动态分发时使用 `BoxedAgent`（见下方）
- **不可命名的 Stream 类型**：RPITIT 的返回类型不可命名，无法存到结构体字段。仅在框架内部状态机存储时做局部 `Pin<Box<dyn Stream>>`，用户可见接口零擦除

## AgentExt 扩展方法

通过 blanket impl 自动提供给所有 `Agent`：

```rust
pub trait AgentExt: Agent + Sized {
    /// 映射 stream 中的每个 Response item
    fn map_response<F, R>(self, f: F) -> MapResponse<Self, F>
    where F: Fn(Self::Response) -> R;

    /// 转换 Request 类型
    fn map_request<F, NewReq>(self, f: F) -> MapRequest<Self, F>
    where F: Fn(NewReq) -> Self::Request;

    /// 转换 Error 类型
    fn map_err<F, NewErr>(self, f: F) -> MapErr<Self, F>
    where F: Fn(Self::Error) -> NewErr;

    /// 应用一个 Layer 适配器
    fn layer<L: Layer<Self>>(self, layer: L) -> L::Output;

    /// 类型擦除——仅在需要 dyn 时使用
    fn boxed(self) -> BoxedAgent<Self::Request, Self::Response, Self::Error>
    where Self: 'static;
}

impl<A: Agent> AgentExt for A {}
```

## Layer Trait

```rust
pub trait Layer<A: Agent> {
    type Output: Agent;
    fn layer(self, inner: A) -> Self::Output;
}
```

Layer 将一个 Agent 包裹为新的 Agent，可以叠加多层：

```rust
let agent = openai_client
    .map_response(|chunk| MyEvent::from(chunk))
    .map_err(|e| MyError::from(e))
    .layer(LoggingLayer);
```

## BoxedAgent（可选，动态分发）

```rust
/// 类型擦除的 Agent，用于需要存到集合或 dyn dispatch 的场景
/// 这是唯一做 Box 擦除的地方，用户主动 opt-in
pub struct BoxedAgent<Req, Resp, Err> { ... }

impl<Req, Resp, Err> Agent for BoxedAgent<Req, Resp, Err> {
    type Request = Req;
    type Response = Resp;
    type Error = Err;
    // 内部用 BoxFuture + BoxStream
}
```

用户通过 `.boxed()` 主动选择类型擦除，框架不强制。
