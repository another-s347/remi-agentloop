use futures::StreamExt;
use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;
use serde_json::json;

#[tool]
async fn sample_search(query: String, limit: Option<u32>, exact: bool) -> String {
    format!("{query}:{:?}:{exact}", limit)
}

#[test]
fn tool_macro_generates_schemars_schema() {
    let schema = SampleSearch::new().parameters_schema();
    let properties = schema["properties"].as_object().unwrap();
    let required = schema["required"].as_array().unwrap();

    assert_eq!(schema["type"], "object");
    assert_eq!(properties["query"]["type"], "string");
    assert_eq!(properties["exact"]["type"], "boolean");
    assert!(required.iter().any(|value| value == "query"));
    assert!(required.iter().any(|value| value == "exact"));
    assert!(!required.iter().any(|value| value == "limit"));
}

#[test]
fn tool_macro_executes_with_typed_arguments() {
    let result = futures::executor::block_on(async {
        let tool = SampleSearch::new();
        let ctx = ChatCtx::default();
        let output = tool
            .execute(
                json!({
                    "query": "rust",
                    "limit": 3,
                    "exact": true
                }),
                None,
                ctx,
            )
            .await
            .unwrap();

        match output {
            ToolResult::Output(stream) => {
                let mut stream = std::pin::pin!(stream);
                let mut final_text = String::new();
                while let Some(item) = stream.next().await {
                    if let ToolOutput::Result(Content::Text(text)) = item {
                        final_text = text;
                    }
                }
                final_text
            }
            ToolResult::Interrupt(_) => panic!("unexpected interrupt"),
        }
    });

    assert_eq!(result, "rust:Some(3):true");
}