use std::env;
use std::path::PathBuf;
use std::time::Instant;

use futures::StreamExt;
use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::ProtocolEvent;
use remi_agentloop::types::LoopInput;
use remi_agentloop_wasm::WasmAgent;
use serde::Serialize;

#[derive(Clone, Debug)]
struct BenchmarkConfig {
    wasm_path: PathBuf,
    total_tokens: usize,
    chunk_tokens: usize,
    warmup_rounds: usize,
    measured_rounds: usize,
    json: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RoundMetrics {
    round: usize,
    total_tokens: usize,
    chunk_tokens: usize,
    total_events: usize,
    observed_tps: f64,
    batch_ready_ms: f64,
    first_event_ms: Option<f64>,
    total_ms: f64,
    per_event_us: f64,
}

#[derive(Clone, Debug, Serialize)]
struct AggregateMetrics {
    total_tokens: usize,
    chunk_tokens: usize,
    rounds: usize,
    average_observed_tps: f64,
    average_batch_ready_ms: f64,
    average_first_event_ms: f64,
    average_total_ms: f64,
    average_per_event_us: f64,
}

#[derive(Clone, Debug, Serialize)]
struct BenchmarkReport {
    mode: &'static str,
    wasm_path: String,
    warmup_rounds: usize,
    measured_rounds: usize,
    measured: Vec<RoundMetrics>,
    aggregate: AggregateMetrics,
}

#[tokio::main]
async fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            print_usage_and_exit(2);
        }
    };

    let agent = WasmAgent::from_file(&config.wasm_path).expect("failed to load benchmark WASM component");

    for _ in 0..config.warmup_rounds {
        let _ = run_round(&agent, config.total_tokens, config.chunk_tokens).await;
    }

    let mut measured = Vec::with_capacity(config.measured_rounds);
    for round_index in 0..config.measured_rounds {
        let mut round = run_round(&agent, config.total_tokens, config.chunk_tokens).await;
        round.round = round_index + 1;
        measured.push(round);
    }

    let aggregate = aggregate_metrics(config.total_tokens, config.chunk_tokens, &measured);
    let report = BenchmarkReport {
        mode: "wasm_component_batch_host",
        wasm_path: config.wasm_path.display().to_string(),
        warmup_rounds: config.warmup_rounds,
        measured_rounds: config.measured_rounds,
        measured,
        aggregate,
    };

    if config.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("failed to serialize benchmark report")
        );
        return;
    }

    print_report(&report);
}

async fn run_round(agent: &WasmAgent, total_tokens: usize, chunk_tokens: usize) -> RoundMetrics {
    let spec_json = serde_json::json!({
        "total_tokens": total_tokens,
        "chunk_tokens": chunk_tokens,
    })
    .to_string();

    let start = Instant::now();
    let stream = agent
        .chat(LoopInput::start(spec_json))
        .await
        .expect("agent.chat failed");
    let batch_ready = start.elapsed();
    let mut stream = std::pin::pin!(stream);

    let mut first_event_at = None;
    let mut output_tokens = 0usize;
    let mut total_events = 0usize;

    while let Some(event) = stream.next().await {
        total_events += 1;
        if first_event_at.is_none() {
            first_event_at = Some(start.elapsed());
        }
        if let ProtocolEvent::Delta { content, .. } = event {
            output_tokens += content.len();
        }
    }

    let total_elapsed = start.elapsed();
    let total_ms = total_elapsed.as_secs_f64() * 1000.0;
    let observed_tps = if total_elapsed.is_zero() {
        0.0
    } else {
        output_tokens as f64 / total_elapsed.as_secs_f64()
    };

    RoundMetrics {
        round: 0,
        total_tokens: output_tokens,
        chunk_tokens,
        total_events,
        observed_tps,
        batch_ready_ms: batch_ready.as_secs_f64() * 1000.0,
        first_event_ms: first_event_at.map(|value| value.as_secs_f64() * 1000.0),
        total_ms,
        per_event_us: if total_events == 0 {
            0.0
        } else {
            total_ms * 1000.0 / total_events as f64
        },
    }
}

fn aggregate_metrics(total_tokens: usize, chunk_tokens: usize, rounds: &[RoundMetrics]) -> AggregateMetrics {
    let rounds_count = rounds.len().max(1) as f64;
    let avg = |selector: fn(&RoundMetrics) -> f64| -> f64 {
        rounds.iter().map(selector).sum::<f64>() / rounds_count
    };
    let average_first_event_ms = if rounds.is_empty() {
        0.0
    } else {
        rounds
            .iter()
            .filter_map(|round| round.first_event_ms)
            .sum::<f64>()
            / rounds_count
    };

    AggregateMetrics {
        total_tokens,
        chunk_tokens,
        rounds: rounds.len(),
        average_observed_tps: avg(|round| round.observed_tps),
        average_batch_ready_ms: avg(|round| round.batch_ready_ms),
        average_first_event_ms,
        average_total_ms: avg(|round| round.total_ms),
        average_per_event_us: avg(|round| round.per_event_us),
    }
}

fn parse_args() -> Result<BenchmarkConfig, String> {
    let mut config = BenchmarkConfig {
        wasm_path: PathBuf::from("examples/wasm-benchmark-guest/benchmark_guest.wasm"),
        total_tokens: 1000,
        chunk_tokens: 5,
        warmup_rounds: 1,
        measured_rounds: 5,
        json: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--wasm" => {
                let value = args.next().ok_or_else(|| "missing value for --wasm".to_string())?;
                config.wasm_path = PathBuf::from(value);
            }
            "--tokens" => {
                config.total_tokens = parse_usize_arg("--tokens", args.next())?;
            }
            "--chunk-tokens" => {
                config.chunk_tokens = parse_usize_arg("--chunk-tokens", args.next())?;
            }
            "--warmup" => {
                config.warmup_rounds = parse_usize_arg("--warmup", args.next())?;
            }
            "--rounds" => {
                config.measured_rounds = parse_usize_arg("--rounds", args.next())?;
            }
            "--json" => {
                config.json = true;
            }
            "--help" | "-h" => {
                print_usage_and_exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if config.total_tokens == 0 {
        return Err("--tokens must be greater than zero".into());
    }
    if config.chunk_tokens == 0 {
        return Err("--chunk-tokens must be greater than zero".into());
    }
    if config.measured_rounds == 0 {
        return Err("--rounds must be greater than zero".into());
    }

    Ok(config)
}

fn parse_usize_arg(flag: &str, value: Option<String>) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("missing value for {flag}"))?;
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid integer for {flag}: {value}"))
}

fn print_report(report: &BenchmarkReport) {
    println!("mode: {}", report.mode);
    println!("wasm_path: {}", report.wasm_path);
    println!(
        "warmup_rounds: {} | measured_rounds: {}",
        report.warmup_rounds, report.measured_rounds
    );
    println!(
        "observed_tps(avg) | batch_ready_ms(avg) | first_event_ms(avg) | total_ms(avg) | per_event_us(avg)"
    );
    println!(
        "{:>17.2} | {:>19.2} | {:>19.2} | {:>13.2} | {:>17.2}",
        report.aggregate.average_observed_tps,
        report.aggregate.average_batch_ready_ms,
        report.aggregate.average_first_event_ms,
        report.aggregate.average_total_ms,
        report.aggregate.average_per_event_us,
    );

    println!();
    println!("Per-round detail:");
    for round in &report.measured {
        println!(
            "  round={} observed_tps={:.2} batch_ready_ms={:.2} first_event_ms={:.2} total_ms={:.2} total_events={} per_event_us={:.2}",
            round.round,
            round.observed_tps,
            round.batch_ready_ms,
            round.first_event_ms.unwrap_or(0.0),
            round.total_ms,
            round.total_events,
            round.per_event_us,
        );
    }
    println!();
    println!("note: this path is batch-return, not true streaming; batch_ready_ms is the primary wasm host metric.");
}

fn print_usage_and_exit(code: i32) -> ! {
    eprintln!(
        "Usage: cargo run -p remi-agentloop-wasm --example wasm_component_benchmark -- [options]\n\n\
Options:\n\
  --wasm examples/wasm-benchmark-guest/benchmark_guest.wasm\n\
                       Path to a built benchmark component\n\
  --tokens 1000        Total mock tokens to emit inside the guest\n\
  --chunk-tokens 5     Tokens packed into each guest delta event\n\
  --warmup 1           Warmup rounds before measurement\n\
  --rounds 5           Measured rounds\n\
  --json               Emit JSON instead of a text table\n"
    );
    std::process::exit(code);
}