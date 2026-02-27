# Model 层

> ChatModel trait、OpenAIClient 实现、SSE 解析

## ChatModel

`ChatModel` 是一个 marker trait / supertrait alias，任何 Agent 只要关联类型匹配就自动满足：

```rust
/// 任何 Agent 只要满足此约束就是 ChatModel
pub trait ChatModel:
    Agent<Request = ChatRequest, Response = ChatResponseChunk, Error = AgentError>
{
}

// blanket impl
impl<T> ChatModel for T
where
    T: Agent<Request = ChatRequest, Response = ChatResponseChunk, Error = AgentError>,
{
}
```

## OpenAIClient (model/openai.rs)

```rust
pub struct OpenAIClient {
    client: reqwest::Client,
    base_url: String,    // 默认 "https://api.openai.com/v1"
    api_key: String,
    model: String,       // 默认 model name
}

impl OpenAIClient {
    pub fn new(api_key: impl Into<String>) -> Self;
    pub fn with_base_url(self, url: impl Into<String>) -> Self;
    pub fn with_model(self, model: impl Into<String>) -> Self;

    /// 从 AgentConfig 构造（WASM 友好——不需要编译时确定 key）
    pub fn from_config(config: &AgentConfig) -> Self;
}

impl Agent for OpenAIClient {
    type Request = ChatRequest;
    type Response = ChatResponseChunk;
    type Error = AgentError;

    fn chat(&self, req: ChatRequest)
        -> impl Future<Output = Result<impl Stream<Item = ChatResponseChunk>, AgentError>>
    {
        async move {
            let response = self.client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(&self.api_key)
                .json(&req)
                .send()
                .await?;

            // 检查 HTTP 状态
            if !response.status().is_success() { ... }

            // 返回 SSE 解析 stream
            Ok(stream! {
                let mut reader = BufReader::new(response.bytes_stream());
                // 逐行读取 "data: {...}" SSE 事件
                // 解析为 ChatResponseChunk 各变体
                // yield chunk;
            })
        }
    }
}
```

## SSE 解析逻辑

1. 按行读取 response body stream
2. 跳过空行和 `: ` 注释行
3. `data: [DONE]` → yield `ChatResponseChunk::Done`
4. `data: {...}` → JSON 解析为 OpenAI 的 `ChatCompletionChunk` 结构
5. 根据 `choices[0].delta` 的内容映射为 `ChatResponseChunk` 的各个变体

### 兼容性

通过自定义 `base_url` 支持：
- **OpenAI API** — `https://api.openai.com/v1`
- **Azure OpenAI** — `https://{resource}.openai.azure.com/openai/deployments/{deployment}`
- **Ollama** — `http://localhost:11434/v1`
- **vLLM** — `http://localhost:8000/v1`
- 其他 OpenAI 兼容 API
