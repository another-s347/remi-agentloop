//! Multimodal image understanding example
//!
//! Demonstrates sending an image URL with text to the model using the
//! unified `LoopInput` + `Content::Parts` multimodal support.
//!
//! Run with:
//!   REMI_API_KEY=... REMI_BASE_URL=... REMI_MODEL=kimi-k2.5-0711-preview \
//!     cargo run --example multimodal_image --features http-client

use futures::StreamExt;
use remi_agentloop::prelude::*;

#[tokio::main]
async fn main() -> Result<(), AgentError> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("REMI_API_KEY"))
        .expect("OPENAI_API_KEY or REMI_API_KEY must be set");

    let model =
        std::env::var("REMI_MODEL").unwrap_or_else(|_| "kimi-k2.5-0711-preview".to_string());
    let base_url = std::env::var("REMI_BASE_URL")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .ok();

    let mut oai = OpenAIClient::new(api_key).with_model(model.clone());
    if let Some(url) = base_url {
        oai = oai.with_base_url(url);
    }

    let agent = AgentBuilder::new()
        .model(oai)
        .system("You are a helpful vision assistant. Describe images accurately and concisely.")
        .max_turns(1)
        .build();

    // ── Build multimodal input with image (base64-encoded) ──────────────
    // Generate a tiny 2x2 red PNG for testing (avoids network issues)
    // Or load from local file / env var
    let (image_description, data_url) = if let Ok(path) = std::env::var("REMI_IMAGE_PATH") {
        let bytes = std::fs::read(&path)
            .map_err(|e| AgentError::other(format!("failed to read image: {e}")))?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let ext = path.rsplit('.').next().unwrap_or("png");
        let mime = match ext {
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => "image/png",
        };
        (path, format!("data:{mime};base64,{b64}"))
    } else if let Ok(url) = std::env::var("REMI_IMAGE_URL") {
        println!("Downloading image from {url}...");
        let img_bytes = reqwest::get(&url)
            .await
            .map_err(|e| AgentError::other(format!("failed to download image: {e}")))?
            .bytes()
            .await
            .map_err(|e| AgentError::other(format!("failed to read image bytes: {e}")))?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&img_bytes);
        (url, format!("data:image/png;base64,{b64}"))
    } else {
        // Default: load test image from examples directory
        let default_path = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/test_image.png");
        let bytes = std::fs::read(default_path)
            .map_err(|e| AgentError::other(format!("failed to read {default_path}: {e}")))?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        (
            format!("{default_path} (64x64 gradient)"),
            format!("data:image/png;base64,{b64}"),
        )
    };

    println!("Multimodal Image Understanding — model: {model}");
    println!("Image: {image_description}");
    println!("{}", "─".repeat(60));

    let input = LoopInput::start_content(Content::parts(vec![
        ContentPart::text("这张图片里有什么？请用中文简要描述。"),
        ContentPart::image_url(&data_url),
    ]));

    let stream = agent.chat(input).await?;
    let mut stream = std::pin::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(text) => {
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            AgentEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                eprintln!("\n[tokens: prompt={prompt_tokens} completion={completion_tokens}]");
            }
            AgentEvent::Done => {
                println!();
                println!("{}", "─".repeat(60));
                println!("Done.");
            }
            AgentEvent::Error(e) => {
                eprintln!("\nError: {e}");
            }
            _ => {}
        }
    }

    Ok(())
}
