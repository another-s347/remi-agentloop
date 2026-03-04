//! Tavily web search tool.
//!
//! Calls the Tavily Search API (`https://api.tavily.com/search`) and returns
//! formatted results with title, URL, and a content snippet.
//!
//! Set `TAVILY_API_KEY` env var or pass the key directly via [`TavilySearchTool::new`].

use async_stream::stream;
use futures::Stream;

use remi_core::error::AgentError;
use remi_core::tool::{Tool, ToolContext, ToolOutput, ToolResult};
use remi_core::types::ResumePayload;

const TAVILY_API_URL: &str = "https://api.tavily.com/search";

/// Web search via the Tavily Search API.
///
/// # Usage
/// ```no_run
/// use remi_deepagent::TavilySearchTool;
///
/// let tool = TavilySearchTool::new("tvly-...");
/// // or from env var:
/// let tool = TavilySearchTool::from_env().expect("TAVILY_API_KEY not set");
/// ```
pub struct TavilySearchTool {
    api_key: String,
    max_results: usize,
    search_depth: String, // "basic" | "advanced"
}

impl TavilySearchTool {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            max_results: 5,
            search_depth: "basic".to_string(),
        }
    }

    /// Read API key from `TAVILY_API_KEY` env var.
    pub fn from_env() -> Option<Self> {
        std::env::var("TAVILY_API_KEY").ok().map(Self::new)
    }

    pub fn max_results(mut self, n: usize) -> Self {
        self.max_results = n;
        self
    }

    /// Use `"advanced"` for deeper search (uses more API credits).
    pub fn search_depth(mut self, depth: impl Into<String>) -> Self {
        self.search_depth = depth.into();
        self
    }
}

impl Tool for TavilySearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str {
        "Search the web using Tavily. \
         Returns a list of relevant results with titles, URLs, and content snippets. \
         Use for current events, documentation look-ups, or any topic requiring fresh web data."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query string"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        _ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl Stream<Item = ToolOutput>>, AgentError>>
    {
        let api_key = self.api_key.clone();
        let default_max = self.max_results;
        let search_depth = self.search_depth.clone();

        async move {
            let query = arguments["query"]
                .as_str()
                .ok_or_else(|| AgentError::tool("web_search", "missing 'query'"))?
                .to_string();

            let max_results = arguments["max_results"]
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or(default_max);

            Ok(ToolResult::Output(stream! {
                let client = reqwest::Client::new();
                let body = serde_json::json!({
                    "api_key": api_key,
                    "query": query,
                    "search_depth": search_depth,
                    "max_results": max_results,
                    "include_answer": true,
                    "include_raw_content": false,
                });

                let resp = match client
                    .post(TAVILY_API_URL)
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        yield ToolOutput::Result(format!("error contacting Tavily: {}", e));
                        return;
                    }
                };

                let status = resp.status();
                let text = match resp.text().await {
                    Ok(t) => t,
                    Err(e) => {
                        yield ToolOutput::Result(format!("error reading Tavily response: {}", e));
                        return;
                    }
                };

                if !status.is_success() {
                    yield ToolOutput::Result(format!("Tavily API error {}: {}", status, text));
                    return;
                }

                let json: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        yield ToolOutput::Result(format!("failed to parse Tavily response: {}", e));
                        return;
                    }
                };

                let mut output = String::new();

                // Include the AI-generated answer if present
                if let Some(answer) = json["answer"].as_str() {
                    if !answer.is_empty() {
                        output.push_str("**Summary:** ");
                        output.push_str(answer);
                        output.push_str("\n\n");
                    }
                }

                // Format results
                if let Some(results) = json["results"].as_array() {
                    for (i, result) in results.iter().enumerate() {
                        let title   = result["title"].as_str().unwrap_or("(no title)");
                        let url     = result["url"].as_str().unwrap_or("");
                        let content = result["content"].as_str().unwrap_or("");
                        output.push_str(&format!("{}. **{}**\n   {}\n   {}\n\n",
                            i + 1, title, url, content));
                    }
                }

                if output.is_empty() {
                    output = "No results found.".to_string();
                }

                yield ToolOutput::Result(output);
            }))
        }
    }
}
