# 适配器设计

> MapResponse、MapRequest、MapErr、TransformStream、RetryLayer、LoggingLayer

适配器是包裹内部 Agent 的 struct，自身也实现 `Agent` trait。通过 `AgentExt` 上的链式方法或 `Layer` trait 使用。

## MapResponse

对 stream 中每个 item 做映射：

```rust
pub struct MapResponse<A, F> { inner: A, f: F }

impl<A: Agent, F: Fn(A::Response) -> R, R> Agent for MapResponse<A, F> {
    type Request = A::Request;
    type Response = R;        // ← 强类型映射
    type Error = A::Error;

    fn chat(&self, req: Self::Request)
        -> impl Future<Output = Result<impl Stream<Item = R>, Self::Error>>
    {
        async move {
            let stream = self.inner.chat(req).await?;
            Ok(stream.map(|item| (self.f)(item)))
        }
    }
}
```

## MapRequest

转换 Request 类型：

```rust
pub struct MapRequest<A, F> { inner: A, f: F }

impl<A: Agent, F: Fn(NewReq) -> A::Request, NewReq> Agent for MapRequest<A, F> {
    type Request = NewReq;    // ← 新的请求类型
    type Response = A::Response;
    type Error = A::Error;

    fn chat(&self, req: NewReq)
        -> impl Future<Output = Result<impl Stream<Item = A::Response>, Self::Error>>
    {
        self.inner.chat((self.f)(req))
    }
}
```

## MapErr

转换 Error 类型：

```rust
pub struct MapErr<A, F> { inner: A, f: F }

impl<A: Agent, F: Fn(A::Error) -> NewErr, NewErr> Agent for MapErr<A, F> {
    type Request = A::Request;
    type Response = A::Response;
    type Error = NewErr;      // ← 新的错误类型

    fn chat(&self, req: Self::Request)
        -> impl Future<Output = Result<impl Stream<Item = Self::Response>, NewErr>>
    {
        async move { self.inner.chat(req).await.map_err(&self.f) }
    }
}
```

## TransformStream

整体变换 stream（如 filter、buffer、throttle）：

```rust
pub struct TransformStream<A, F> { inner: A, f: F }

impl<A, F, NewStream> Agent for TransformStream<A, F>
where
    A: Agent,
    F: Fn(/* A's stream */) -> NewStream,  // 接受内部 stream，返回新 stream
    NewStream: Stream,
{
    type Request = A::Request;
    type Response = NewStream::Item;
    type Error = A::Error;
    // ...
}
```

> **注意**：由于 RPITIT 的 stream 类型不可命名，`TransformStream` 的 `F` 签名需要用泛型 + 闭包推导。实现时可能需要 `F: Fn(Pin<Box<dyn Stream<Item = A::Response>>>) -> NewStream` 做局部擦除。这是 RPITIT 唯一需要权衡的地方。

## RetryLayer

```rust
pub struct RetryLayer { max_retries: usize }
pub struct RetryAgent<A> { inner: A, max_retries: usize }

impl<A: Agent> Agent for RetryAgent<A>
where A::Error: /* 可判断是否可重试 */
{
    // chat() 内部循环调用 inner.chat()，直到成功或耗尽重试次数
    // 仅重试 Future（连接阶段），不重试已开始的 stream
}
```

## LoggingLayer

```rust
pub struct LoggingLayer;
pub struct LoggingAgent<A> { inner: A }

impl<A: Agent> Agent for LoggingAgent<A>
where A::Request: Debug, A::Response: Debug
{
    // chat() 前打印请求，stream 每个 item 打印
}
```
