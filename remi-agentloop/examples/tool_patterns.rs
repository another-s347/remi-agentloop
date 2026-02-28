/// `#[tool]` 宏所有用法示例
///
/// 展示 7 种模式：
/// 1. 最简 — 纯参数，自动包装
/// 2. 带 ctx — 读写 user_state
/// 3. 带 resume — 中断恢复
/// 4. 返回 ToolResult<T> — 主动发起中断
/// 5. 返回 ToolResult<impl Stream> — 完全自定义流
/// 6. 渐进式披露 — enabled() + user_state
/// 7. 手动 impl Tool — 最大灵活性
use remi_agentloop::prelude::*;
use remi_agentloop::tool_macro as tool;

// ═══════════════════════════════════════════════════════════════════════════════
// 1. 最简用法 — 纯参数，自动 to_string() 包装
// ═══════════════════════════════════════════════════════════════════════════════

/// Add two numbers together.
#[tool]
async fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// Greet a user by name.
#[tool]
async fn greet(name: String) -> String {
    format!("Hello, {name}!")
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2. 带 ctx 参数 — 读写 user_state（渐进式状态）
// ═══════════════════════════════════════════════════════════════════════════════

/// Search the web for information. Marks search as done in user_state.
#[tool]
async fn web_search(query: String, ctx: &ToolContext) -> String {
    // 读 user_state
    let prev_count = {
        let us = ctx.user_state.read().unwrap();
        us["search_count"].as_u64().unwrap_or(0)
    };

    // 写 user_state — 标记搜索完成，后续 tool 可据此解锁
    {
        let mut us = ctx.user_state.write().unwrap();
        us["search_done"] = serde_json::json!(true);
        us["search_count"] = serde_json::json!(prev_count + 1);
        us["last_query"] = serde_json::json!(query);
    }

    // 也可以读 ctx 的其他字段
    let _thread = &ctx.thread_id;
    let _run = &ctx.run_id;
    let _model = &ctx.config.model;

    format!("Found results for: {query} (search #{})", prev_count + 1)
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. 带 resume 参数 — 处理中断恢复
//    （注意：这里只是展示 resume 参数可用，实际中断靠返回 ToolResult::Interrupt）
// ═══════════════════════════════════════════════════════════════════════════════

/// Process a payment. If resumed, use the approval result.
#[tool]
async fn process_payment(amount: f64, resume: Option<ResumePayload>) -> String {
    if let Some(payload) = resume {
        // 中断后恢复 — payload.result 包含外部系统的审批结果
        let approved = payload.result["approved"].as_bool().unwrap_or(false);
        if approved {
            format!("Payment of ${amount:.2} approved and processed!")
        } else {
            format!("Payment of ${amount:.2} was rejected.")
        }
    } else {
        // 首次调用 — 正常处理（简单场景直接返回结果）
        format!("Payment of ${amount:.2} processed immediately.")
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. 返回 ToolResult<T> — 主动发起中断
//    宏检测到 ToolResult 返回类型，自动 match Output/Interrupt
// ═══════════════════════════════════════════════════════════════════════════════

/// Transfer funds — requires human approval for amounts > $1000.
#[tool]
async fn transfer_funds(
    amount: f64,
    to_account: String,
    resume: Option<ResumePayload>,
) -> ToolResult<String> {
    if let Some(payload) = resume {
        // 恢复路径：外部审批完成
        let approved = payload.result["approved"].as_bool().unwrap_or(false);
        if approved {
            ToolResult::Output(format!("✓ Transferred ${amount:.2} to {to_account}"))
        } else {
            ToolResult::Output(format!("✗ Transfer to {to_account} was denied"))
        }
    } else if amount > 1000.0 {
        // 大额转账 → 发起中断，等待人类审批
        ToolResult::Interrupt(InterruptRequest::new(
            "human_approval",
            serde_json::json!({
                "action": "transfer",
                "amount": amount,
                "to_account": to_account,
                "message": format!("Approve transfer of ${amount:.2} to {to_account}?"),
            }),
        ))
    } else {
        // 小额直接通过
        ToolResult::Output(format!("✓ Transferred ${amount:.2} to {to_account}"))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 5. 返回 ToolResult<impl Stream<Item = ToolOutput>> — 完全自定义流
//    适用于需要流式增量输出的场景（如长时间任务、进度反馈）
// ═══════════════════════════════════════════════════════════════════════════════

/// Execute a long-running analysis with streaming progress updates.
#[tool]
async fn analyze_data(
    dataset: String,
    ctx: &ToolContext,
) -> ToolResult<impl futures::Stream<Item = ToolOutput>> {
    // 也可以在流式 tool 里写 user_state
    {
        let mut us = ctx.user_state.write().unwrap();
        us["analysis_running"] = serde_json::json!(true);
    }

    ToolResult::Output(async_stream::stream! {
        // 流式发送进度
        yield ToolOutput::Delta(format!("Loading dataset: {dataset}...\n"));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        yield ToolOutput::Delta("Preprocessing data...\n".to_string());

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        yield ToolOutput::Delta("Running analysis...\n".to_string());

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 最终结果
        yield ToolOutput::Result(format!("Analysis complete: {dataset} contains 42 records."));
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// 6. 渐进式披露 — 需要手动 impl（因为 enabled() 不在宏生成范围内）
//    搜索完成后才解锁 summarize tool
// ═══════════════════════════════════════════════════════════════════════════════

/// Summarize the search results (only available after search is done).
struct SummarizeTool;

impl Tool for SummarizeTool {
    fn name(&self) -> &str { "summarize" }
    fn description(&self) -> &str { "Summarize previous search results. Only available after a search." }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "max_length": { "type": "integer", "description": "Max summary length" }
            },
            "required": []
        })
    }

    /// 渐进式披露核心：只有 user_state["search_done"] == true 时才启用
    fn enabled(&self, user_state: &serde_json::Value) -> bool {
        user_state["search_done"].as_bool().unwrap_or(false)
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        _resume: Option<ResumePayload>,
        ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl futures::Stream<Item = ToolOutput>>, AgentError>> {
        async move {
            let last_query = {
                let us = ctx.user_state.read().unwrap();
                us["last_query"].as_str().unwrap_or("unknown").to_string()
            };
            let max_len = arguments["max_length"].as_u64().unwrap_or(200);

            let summary = format!(
                "Summary of search for '{}' (max {} chars): ...",
                last_query, max_len
            );

            Ok(ToolResult::Output(async_stream::stream! {
                yield ToolOutput::Result(summary);
            }))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 7. 手动 impl Tool — 最大灵活性（流式 + 中断 + 状态 + enabled 全部自定义）
// ═══════════════════════════════════════════════════════════════════════════════

/// A tool that demonstrates all features: streaming + interrupt + state + enabled.
struct FullFeatureTool;

impl Tool for FullFeatureTool {
    fn name(&self) -> &str { "full_feature" }
    fn description(&self) -> &str { "Demonstrates streaming output, interrupt, and state mutation." }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["stream", "interrupt", "quick"] }
            },
            "required": ["action"]
        })
    }

    fn enabled(&self, user_state: &serde_json::Value) -> bool {
        // 只在 analysis_running 不为 true 时启用（避免并发分析）
        !user_state["analysis_running"].as_bool().unwrap_or(false)
    }

    fn execute(
        &self,
        arguments: serde_json::Value,
        resume: Option<ResumePayload>,
        ctx: &ToolContext,
    ) -> impl std::future::Future<Output = Result<ToolResult<impl futures::Stream<Item = ToolOutput>>, AgentError>> {
        async move {
            let action = arguments["action"].as_str().unwrap_or("quick").to_string();

            // 中断请求 — 唯一的 non-Output 路径, 提前返回
            if resume.is_none() && action == "interrupt" {
                return Ok(ToolResult::Interrupt(InterruptRequest::new(
                    "confirmation",
                    serde_json::json!({"message": "Please confirm this action"}),
                )));
            }

            // 写 user_state
            {
                let mut us = ctx.user_state.write().unwrap();
                us["last_action"] = serde_json::json!(&action);
            }

            // 所有 Output 路径统一在一个 stream 里：resume / stream / quick
            let is_stream = action == "stream";
            let resume_val = resume.map(|p| p.result.to_string());

            Ok(ToolResult::Output(async_stream::stream! {
                if let Some(val) = resume_val {
                    yield ToolOutput::Result(format!("Resumed with: {val}"));
                } else if is_stream {
                    for i in 1..=5 {
                        yield ToolOutput::Delta(format!("Step {i}/5...\n"));
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    yield ToolOutput::Result("All 5 steps complete!".to_string());
                } else {
                    yield ToolOutput::Result(format!("Quick action done: {}", action));
                }
            }))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4+5 的组合: 带 ctx + resume + ToolResult 中断 + 流式进度
// ═══════════════════════════════════════════════════════════════════════════════

/// Deploy a service — requires approval, then streams progress.
#[tool]
async fn deploy_service(
    service_name: String,
    environment: String,
    ctx: &ToolContext,
    resume: Option<ResumePayload>,
) -> ToolResult<impl futures::Stream<Item = ToolOutput>> {
    if let Some(payload) = resume {
        let approved = payload.result["approved"].as_bool().unwrap_or(false);

        // 审批通过 → 流式部署进度；否则取消
        // 写入 user_state
        if approved {
            let mut us = ctx.user_state.write().unwrap();
            us["deploying"] = serde_json::json!(true);
            us["deploy_target"] = serde_json::json!(format!("{service_name}@{environment}"));
        }

        ToolResult::Output(async_stream::stream! {
            if approved {
                yield ToolOutput::Delta(format!("Deploying {service_name} to {environment}...\n"));
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                yield ToolOutput::Delta("Building image...\n".to_string());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                yield ToolOutput::Delta("Pushing to registry...\n".to_string());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                yield ToolOutput::Delta("Rolling out...\n".to_string());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                yield ToolOutput::Result(format!("✓ {service_name} deployed to {environment} successfully!"));
            } else {
                yield ToolOutput::Result("Deployment cancelled by operator.".to_string());
            }
        })
    } else {
        // 首次调用 → 需要审批
        ToolResult::Interrupt(InterruptRequest::new(
            "deploy_approval",
            serde_json::json!({
                "service": service_name,
                "environment": environment,
                "message": format!("Approve deployment of {service_name} to {environment}?"),
            }),
        ))
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// main — 仅为编译验证，实际使用需搭配 AgentBuilder
// ═══════════════════════════════════════════════════════════════════════════════

fn main() {
    println!("Tool patterns example — compile-time verification only.");
    println!();
    println!("Available tools (generated by #[tool] macro):");
    println!("  1. Add::new()              — simple: add(a, b) -> f64");
    println!("  2. Greet::new()            — simple: greet(name) -> String");
    println!("  3. WebSearch::new()        — with ctx: writes user_state");
    println!("  4. ProcessPayment::new()   — with resume: handles interrupt resume");
    println!("  5. TransferFunds::new()    — returns ToolResult<String>: interrupt support");
    println!("  6. AnalyzeData::new()      — returns ToolResult<impl Stream>: streaming");
    println!("  7. DeployService::new()    — ctx + resume + interrupt + streaming");
    println!();
    println!("Manual impl tools:");
    println!("  8. SummarizeTool           — enabled() for progressive disclosure");
    println!("  9. FullFeatureTool         — all features combined");
    println!();

    // Verify all macro-generated structs exist and can be constructed
    let _add = Add::new();
    let _greet = Greet::new();
    let _web_search = WebSearch::new();
    let _process_payment = ProcessPayment::new();
    let _transfer_funds = TransferFunds::new();
    let _analyze_data = AnalyzeData::new();
    let _deploy_service = DeployService::new();
    let _summarize = SummarizeTool;
    let _full_feature = FullFeatureTool;

    // Verify they all impl Tool
    fn assert_tool<T: Tool>(_t: &T) {}
    assert_tool(&_add);
    assert_tool(&_greet);
    assert_tool(&_web_search);
    assert_tool(&_process_payment);
    assert_tool(&_transfer_funds);
    assert_tool(&_analyze_data);
    assert_tool(&_deploy_service);
    assert_tool(&_summarize);
    assert_tool(&_full_feature);

    println!("All tools compile and implement Tool trait ✓");
}
