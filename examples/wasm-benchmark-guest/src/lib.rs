use remi_agentloop_guest::prelude::*;
use serde::Deserialize;

#[derive(Default)]
struct BenchmarkGuest;

#[derive(Debug, Deserialize)]
struct BenchmarkSpec {
    total_tokens: usize,
    chunk_tokens: usize,
}

impl GuestAgent for BenchmarkGuest {
    async fn chat(&self, input: LoopInput) -> Result<Vec<ProtocolEvent>, String> {
        let spec = match input {
            LoopInput::Start { content, .. } => {
                serde_json::from_str::<BenchmarkSpec>(&content.text_content())
                    .map_err(|error| format!("invalid benchmark spec: {error}"))?
            }
            LoopInput::Resume { .. } => return Err("resume is not supported by benchmark guest".into()),
        };

        if spec.total_tokens == 0 {
            return Err("total_tokens must be greater than zero".into());
        }
        if spec.chunk_tokens == 0 {
            return Err("chunk_tokens must be greater than zero".into());
        }

        let mut events = Vec::new();
        let mut emitted = 0usize;

        while emitted < spec.total_tokens {
            let next_chunk_tokens = spec
                .chunk_tokens
                .min(spec.total_tokens.saturating_sub(emitted));
            emitted += next_chunk_tokens;

            events.push(ProtocolEvent::Delta {
                content: "x".repeat(next_chunk_tokens),
                role: if emitted == next_chunk_tokens {
                    Some("assistant".into())
                } else {
                    None
                },
            });
        }

        events.push(ProtocolEvent::Usage {
            prompt_tokens: 0,
            completion_tokens: spec.total_tokens as u32,
        });
        events.push(ProtocolEvent::Done);
        Ok(events)
    }
}

remi_agentloop_guest::export_agent!(BenchmarkGuest);