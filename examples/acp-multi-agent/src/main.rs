/// Example: ACP (Agent Communication Protocol) Multi-Agent System
///
/// This example demonstrates:
/// 1. Creating specialized agents with different capabilities
/// 2. Registering agents in an ACP registry
/// 3. Using an ACP router to automatically route tasks to capable agents
/// 4. Agent-to-agent delegation
/// 5. HTTP client/server for remote agent communication

use async_stream::stream;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;

use remi_agentloop_transport::acp::*;

// ── Example: Search Agent ─────────────────────────────────────────────────────

/// A simple search agent that simulates web search.
struct SearchAgent {
    agent_id: AgentId,
}

impl SearchAgent {
    fn new() -> Self {
        Self {
            agent_id: AgentId::new("search_agent"),
        }
    }
}

impl AcpAgent for SearchAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: self.agent_id.clone(),
            name: "Search Agent".into(),
            description: "Specialized in web search and information retrieval".into(),
            version: "1.0.0".into(),
            tools: vec![
                AcpToolDefinition {
                    name: "web_search".into(),
                    description: "Search the web for information".into(),
                    parameters: vec![
                        ToolParameter {
                            name: "query".into(),
                            description: "Search query".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                    ],
                    metadata: HashMap::new(),
                },
            ],
            domains: vec!["search".into(), "web".into(), "information".into()],
            languages: vec!["en".into(), "zh".into()],
            performance: Some(AgentPerformance {
                avg_latency_ms: 500,
                max_concurrency: 10,
                rate_limit_rpm: 60,
            }),
            cost: Some(AgentCost {
                input_cost: 0.001,
                output_cost: 0.002,
                currency: "USD".into(),
            }),
            metadata: HashMap::new(),
        }
    }

    fn execute(
        &self,
        request: AcpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError>> + Send>> {
        let agent_id = self.agent_id.clone();
        let query = request.content.text_content();
        
        Box::pin(async move {
            let task_id = TaskId::new();
            let session_id = request.session_id.unwrap_or_else(SessionId::new);

            Ok(Box::pin(stream! {
                // Start event
                yield AcpEvent::AgentStart {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    agent_name: "Search Agent".into(),
                    metadata: None,
                };

                // Simulate thinking
                yield AcpEvent::ThinkingStart {
                    task_id: task_id.clone(),
                };

                yield AcpEvent::ThinkingDelta {
                    task_id: task_id.clone(),
                    delta: "Analyzing query...".into(),
                };

                yield AcpEvent::ThinkingEnd {
                    task_id: task_id.clone(),
                    content: "I'll search for relevant information.".into(),
                };

                // Simulate tool call
                let tool_call_id = format!("tc_{}", uuid::Uuid::new_v4());
                yield AcpEvent::ToolCallStart {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "web_search".into(),
                    arguments: serde_json::json!({ "query": query }),
                };

                // Simulate search progress
                yield AcpEvent::ToolProgress {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    delta: "Searching web...".into(),
                };

                // Simulate result
                let search_result = format!("Found 3 results for '{}':\n1. Result A\n2. Result B\n3. Result C", query);
                yield AcpEvent::ToolResult {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    result: AcpContent::text(search_result.clone()),
                    error: None,
                };

                // Stream response
                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: "Based on my search, ".into(),
                };

                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: "I found several relevant results. ".into(),
                };

                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: search_result.clone(),
                };

                // Usage
                yield AcpEvent::Usage {
                    task_id: task_id.clone(),
                    input_tokens: 50,
                    output_tokens: 100,
                    cost: 0.0003,
                    currency: "USD".into(),
                };

                // End
                yield AcpEvent::AgentEnd {
                    task_id: task_id.clone(),
                    status: TaskStatus::Success,
                    result: Some(AcpContent::text(search_result)),
                    error: None,
                };
            }) as Pin<Box<dyn Stream<Item = AcpEvent> + Send>>)
        })
    }
}

// ── Example: Code Agent ───────────────────────────────────────────────────────

/// A code analysis and generation agent.
struct CodeAgent {
    agent_id: AgentId,
}

impl CodeAgent {
    fn new() -> Self {
        Self {
            agent_id: AgentId::new("code_agent"),
        }
    }
}

impl AcpAgent for CodeAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: self.agent_id.clone(),
            name: "Code Agent".into(),
            description: "Specialized in code analysis, debugging, and generation".into(),
            version: "1.0.0".into(),
            tools: vec![
                AcpToolDefinition {
                    name: "analyze_code".into(),
                    description: "Analyze code for bugs and improvements".into(),
                    parameters: vec![
                        ToolParameter {
                            name: "code".into(),
                            description: "Code to analyze".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                        ToolParameter {
                            name: "language".into(),
                            description: "Programming language".into(),
                            param_type: "string".into(),
                            required: false,
                            enum_values: Some(vec!["rust".into(), "python".into(), "javascript".into()]),
                        },
                    ],
                    metadata: HashMap::new(),
                },
                AcpToolDefinition {
                    name: "generate_code".into(),
                    description: "Generate code from specification".into(),
                    parameters: vec![
                        ToolParameter {
                            name: "spec".into(),
                            description: "Code specification".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                    ],
                    metadata: HashMap::new(),
                },
            ],
            domains: vec!["code".into(), "programming".into(), "debugging".into()],
            languages: vec!["en".into()],
            performance: Some(AgentPerformance {
                avg_latency_ms: 2000,
                max_concurrency: 5,
                rate_limit_rpm: 30,
            }),
            cost: Some(AgentCost {
                input_cost: 0.01,
                output_cost: 0.03,
                currency: "USD".into(),
            }),
            metadata: HashMap::new(),
        }
    }

    fn execute(
        &self,
        request: AcpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError>> + Send>> {
        let agent_id = self.agent_id.clone();
        let task_description = request.content.text_content();
        
        Box::pin(async move {
            let task_id = TaskId::new();
            let session_id = request.session_id.unwrap_or_else(SessionId::new);

            Ok(Box::pin(stream! {
                // Start
                yield AcpEvent::AgentStart {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    agent_name: "Code Agent".into(),
                    metadata: None,
                };

                // Thinking
                yield AcpEvent::ThinkingStart {
                    task_id: task_id.clone(),
                };

                yield AcpEvent::ThinkingDelta {
                    task_id: task_id.clone(),
                    delta: "Analyzing code task...".into(),
                };

                yield AcpEvent::ThinkingEnd {
                    task_id: task_id.clone(),
                    content: "I'll analyze the code and provide suggestions.".into(),
                };

                // Tool call
                let tool_call_id = format!("tc_{}", uuid::Uuid::new_v4());
                yield AcpEvent::ToolCallStart {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "analyze_code".into(),
                    arguments: serde_json::json!({
                        "code": task_description,
                        "language": "rust"
                    }),
                };

                yield AcpEvent::ToolProgress {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    delta: "Running static analysis...".into(),
                };

                let analysis = "Code analysis complete:\n- No syntax errors\n- 2 style suggestions\n- 1 performance opportunity";
                yield AcpEvent::ToolResult {
                    task_id: task_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    result: AcpContent::text(analysis),
                    error: None,
                };

                // Response
                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: "I've analyzed your code. ".into(),
                };

                yield AcpEvent::ContentDelta {
                    task_id: task_id.clone(),
                    delta: analysis.to_string(),
                };

                // Usage
                yield AcpEvent::Usage {
                    task_id: task_id.clone(),
                    input_tokens: 100,
                    output_tokens: 200,
                    cost: 0.007,
                    currency: "USD".into(),
                };

                // End
                yield AcpEvent::AgentEnd {
                    task_id: task_id.clone(),
                    status: TaskStatus::Success,
                    result: Some(AcpContent::text(format!("Analysis complete. {}", analysis))),
                    error: None,
                };
            }) as Pin<Box<dyn Stream<Item = AcpEvent> + Send>>)
        })
    }
}

// ── Example: Orchestrator Agent (with delegation) ────────────────────────────

/// An orchestrator agent that delegates to specialized agents.
struct OrchestratorAgent {
    agent_id: AgentId,
    router: AcpRouter,
}

impl OrchestratorAgent {
    fn new(router: AcpRouter) -> Self {
        Self {
            agent_id: AgentId::new("orchestrator"),
            router,
        }
    }
}

impl AcpAgent for OrchestratorAgent {
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            agent_id: self.agent_id.clone(),
            name: "Orchestrator Agent".into(),
            description: "Coordinates multiple specialized agents to complete complex tasks".into(),
            version: "1.0.0".into(),
            tools: vec![
                AcpToolDefinition {
                    name: "delegate".into(),
                    description: "Delegate a subtask to a specialized agent".into(),
                    parameters: vec![
                        ToolParameter {
                            name: "agent_type".into(),
                            description: "Type of agent needed (search, code, math, etc.)".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                        ToolParameter {
                            name: "task".into(),
                            description: "Task description".into(),
                            param_type: "string".into(),
                            required: true,
                            enum_values: None,
                        },
                    ],
                    metadata: HashMap::new(),
                },
            ],
            domains: vec!["orchestration".into(), "coordination".into(), "planning".into()],
            languages: vec!["en".into(), "zh".into()],
            performance: Some(AgentPerformance {
                avg_latency_ms: 3000,
                max_concurrency: 20,
                rate_limit_rpm: 100,
            }),
            cost: Some(AgentCost {
                input_cost: 0.005,
                output_cost: 0.015,
                currency: "USD".into(),
            }),
            metadata: HashMap::new(),
        }
    }

    fn execute(
        &self,
        request: AcpRequest,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Pin<Box<dyn Stream<Item = AcpEvent> + Send>>, AcpError>> + Send>> {
        let agent_id = self.agent_id.clone();
        let task_description = request.content.text_content();
        let router = self.router.clone();
        let session_id = request.session_id.unwrap_or_else(SessionId::new);
        
        Box::pin(async move {
            let task_id = TaskId::new();

            Ok(Box::pin(stream! {
                // Start
                yield AcpEvent::AgentStart {
                    session_id: session_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: agent_id.clone(),
                    agent_name: "Orchestrator Agent".into(),
                    metadata: None,
                };

                // Analyze task
                yield AcpEvent::ThinkingStart {
                    task_id: task_id.clone(),
                };

                yield AcpEvent::ThinkingDelta {
                    task_id: task_id.clone(),
                    delta: "Breaking down the complex task...".into(),
                };

                let task_lower = task_description.to_lowercase();
                let needs_search = task_lower.contains("search") || task_lower.contains("find");
                let needs_code = task_lower.contains("code") || task_lower.contains("program");

                let plan = if needs_search && needs_code {
                    "I'll first search for information, then write code based on findings."
                } else if needs_search {
                    "I'll search for the requested information."
                } else if needs_code {
                    "I'll analyze or generate the code."
                } else {
                    "I'll handle this task directly."
                };

                yield AcpEvent::ThinkingDelta {
                    task_id: task_id.clone(),
                    delta: format!(" {}", plan),
                };

                yield AcpEvent::ThinkingEnd {
                    task_id: task_id.clone(),
                    content: plan.to_string(),
                };

                // Execute delegations
                let mut final_result = String::new();

                if needs_search {
                    // Delegate to search agent
                    let delegation_id = DelegationId::new();
                    yield AcpEvent::DelegateStart {
                        delegation_id: delegation_id.clone(),
                        parent_task_id: task_id.clone(),
                        target_agent_id: AgentId::new("search_agent"),
                        target_agent_name: "Search Agent".into(),
                        task_description: task_description.clone(),
                    };

                    // Execute delegation
                    let delegate_req = AcpRequest {
                        session_id: Some(session_id.clone()),
                        content: AcpContent::text(&task_description),
                        target_agent: Some(AgentId::new("search_agent")),
                        routing: None,
                        history: vec![],
                        constraints: None,
                        metadata: HashMap::new(),
                    };

                    match router.execute(delegate_req).await {
                        Ok(mut delegate_stream) => {
                            let mut delegate_result = String::new();
                            while let Some(event) = delegate_stream.next().await {
                                // Forward delegation events
                                yield AcpEvent::DelegateEvent {
                                    delegation_id: delegation_id.clone(),
                                    event: Box::new(event.clone()),
                                };

                                // Collect result
                                if let AcpEvent::ContentDelta { delta, .. } = &event {
                                    delegate_result.push_str(delta);
                                }
                            }

                            yield AcpEvent::DelegateEnd {
                                delegation_id: delegation_id.clone(),
                                result: AcpContent::text(&delegate_result),
                                error: None,
                            };

                            final_result.push_str(&delegate_result);
                        }
                        Err(e) => {
                            yield AcpEvent::DelegateEnd {
                                delegation_id: delegation_id.clone(),
                                result: AcpContent::text(""),
                                error: Some(e.message.clone()),
                            };
                        }
                    }
                }

                if needs_code {
                    // Delegate to code agent
                    let delegation_id = DelegationId::new();
                    yield AcpEvent::DelegateStart {
                        delegation_id: delegation_id.clone(),
                        parent_task_id: task_id.clone(),
                        target_agent_id: AgentId::new("code_agent"),
                        target_agent_name: "Code Agent".into(),
                        task_description: task_description.clone(),
                    };

                    let delegate_req = AcpRequest {
                        session_id: Some(session_id.clone()),
                        content: AcpContent::text(&task_description),
                        target_agent: Some(AgentId::new("code_agent")),
                        routing: None,
                        history: vec![],
                        constraints: None,
                        metadata: HashMap::new(),
                    };

                    match router.execute(delegate_req).await {
                        Ok(mut delegate_stream) => {
                            let mut delegate_result = String::new();
                            while let Some(event) = delegate_stream.next().await {
                                yield AcpEvent::DelegateEvent {
                                    delegation_id: delegation_id.clone(),
                                    event: Box::new(event.clone()),
                                };

                                if let AcpEvent::ContentDelta { delta, .. } = &event {
                                    delegate_result.push_str(delta);
                                }
                            }

                            yield AcpEvent::DelegateEnd {
                                delegation_id: delegation_id.clone(),
                                result: AcpContent::text(&delegate_result),
                                error: None,
                            };

                            if !final_result.is_empty() {
                                final_result.push_str("\n\n");
                            }
                            final_result.push_str(&delegate_result);
                        }
                        Err(e) => {
                            yield AcpEvent::DelegateEnd {
                                delegation_id: delegation_id.clone(),
                                result: AcpContent::text(""),
                                error: Some(e.message.clone()),
                            };
                        }
                    }
                }

                // Final response
                if !final_result.is_empty() {
                    yield AcpEvent::ContentDelta {
                        task_id: task_id.clone(),
                        delta: "\n\nTask completed successfully!".into(),
                    };
                }

                // End
                yield AcpEvent::AgentEnd {
                    task_id: task_id.clone(),
                    status: TaskStatus::Success,
                    result: Some(AcpContent::text(final_result)),
                    error: None,
                };
            }) as Pin<Box<dyn Stream<Item = AcpEvent> + Send>>)
        })
    }
}

// ── Main Example ──────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🤖 ACP Multi-Agent System Example\n");

    // Create agent registry
    let registry = AgentRegistry::new();

    // Create specialized agents
    let search_agent = Box::new(SearchAgent::new());
    let code_agent = Box::new(CodeAgent::new());

    // Create router and register agents
    let router = AcpRouter::new(registry.clone())
        .register_agent(search_agent)
        .register_agent(code_agent);

    // Create orchestrator with access to router
    let orchestrator = Box::new(OrchestratorAgent::new(router.clone()));
    let router = router.register_agent(orchestrator);

    println!("✅ Registered agents:");
    for cap in registry.list() {
        println!("  - {} ({}): {}", cap.name, cap.agent_id, cap.description);
        println!("    Domains: {}", cap.domains.join(", "));
        println!("    Tools: {}", cap.tools.iter().map(|t| &t.name).cloned().collect::<Vec<_>>().join(", "));
    }
    println!();

    // Example 1: Direct agent call (search)
    println!("📝 Example 1: Direct search request");
    let request = AcpRequest {
        session_id: None,
        content: AcpContent::text("search for rust async programming tutorials"),
        target_agent: Some(AgentId::new("search_agent")),
        routing: None,
        history: vec![],
        constraints: None,
        metadata: HashMap::new(),
    };

    let mut stream = router.execute(request).await?;
    while let Some(event) = stream.next().await {
        print_event(&event);
        if matches!(event, AcpEvent::AgentEnd { .. }) {
            break;
        }
    }

    println!("\n{}\n", "=".repeat(80));

    // Example 2: Router-based selection (automatic routing)
    println!("📝 Example 2: Auto-routed request (code task)");
    let request = AcpRequest {
        session_id: None,
        content: AcpContent::text("analyze this rust code for improvements"),
        target_agent: None, // Let router decide
        routing: Some(RoutingHints {
            domains: vec!["code".into()],
            required_tools: vec![],
            language: Some("en".into()),
            cost_preference: None,
            latency_preference: None,
        }),
        history: vec![],
        constraints: None,
        metadata: HashMap::new(),
    };

    let mut stream = router.execute(request).await?;
    while let Some(event) = stream.next().await {
        print_event(&event);
        if matches!(event, AcpEvent::AgentEnd { .. }) {
            break;
        }
    }

    println!("\n{}\n", "=".repeat(80));

    // Example 3: Complex task with delegation (orchestrator)
    println!("📝 Example 3: Complex task with multi-agent delegation");
    let request = AcpRequest {
        session_id: None,
        content: AcpContent::text("search for async patterns and then write rust code implementing them"),
        target_agent: Some(AgentId::new("orchestrator")),
        routing: None,
        history: vec![],
        constraints: Some(ExecutionConstraints {
            timeout_secs: Some(30),
            max_delegation_depth: Some(3),
            max_cost: Some(0.1),
            require_tool_approval: false,
        }),
        metadata: HashMap::new(),
    };

    let mut stream = router.execute(request).await?;
    let mut delegation_depth = 0;
    while let Some(event) = stream.next().await {
        print_event_with_indent(&event, delegation_depth);
        
        match &event {
            AcpEvent::DelegateStart { .. } => delegation_depth += 1,
            AcpEvent::DelegateEnd { .. } => delegation_depth = delegation_depth.saturating_sub(1),
            AcpEvent::AgentEnd { .. } => break,
            _ => {}
        }
    }

    println!("\n{}\n", "=".repeat(80));

    // Example 4: Agent discovery
    println!("📝 Example 4: Agent discovery query");
    let query = AgentQueryRequest {
        domains: vec!["code".into()],
        required_tools: vec!["analyze_code".into()],
        query: Some("code".into()),
        language: Some("en".into()),
    };

    let results = registry.query(&query);
    println!("Found {} matching agents:", results.len());
    for cap in results {
        println!("  - {} ({})", cap.name, cap.agent_id);
        println!("    Domains: {}", cap.domains.join(", "));
        println!("    Tools: {}", cap.tools.iter().map(|t| &t.name).cloned().collect::<Vec<_>>().join(", "));
    }

    Ok(())
}

fn print_event(event: &AcpEvent) {
    print_event_with_indent(event, 0);
}

fn print_event_with_indent(event: &AcpEvent, indent: usize) {
    let prefix = "  ".repeat(indent);
    match event {
        AcpEvent::AgentStart { agent_name, task_id, .. } => {
            println!("{}🤖 Agent: {} (task: {})", prefix, agent_name, task_id);
        }
        AcpEvent::ThinkingStart { .. } => {
            print!("{}💭 ", prefix);
        }
        AcpEvent::ThinkingDelta { delta, .. } => {
            print!("{}", delta);
        }
        AcpEvent::ThinkingEnd { .. } => {
            println!();
        }
        AcpEvent::ToolCallStart { tool_name, arguments, .. } => {
            println!("{}🔧 Tool: {} (args: {})", prefix, tool_name, arguments);
        }
        AcpEvent::ToolProgress { delta, .. } => {
            println!("{}   ⏳ {}", prefix, delta);
        }
        AcpEvent::ToolResult { result, error, .. } => {
            if let Some(err) = error {
                println!("{}   ❌ Error: {}", prefix, err);
            } else {
                println!("{}   ✅ Result: {}", prefix, result.text_content());
            }
        }
        AcpEvent::ContentDelta { delta, .. } => {
            print!("{}", delta);
        }
        AcpEvent::DelegateStart { target_agent_name, task_description, .. } => {
            println!("{}📤 Delegating to {}: {}", prefix, target_agent_name, task_description);
        }
        AcpEvent::DelegateEvent { event, .. } => {
            print_event_with_indent(event, indent + 1);
        }
        AcpEvent::DelegateEnd { result, error, .. } => {
            if let Some(err) = error {
                println!("{}📥 Delegation failed: {}", prefix, err);
            } else {
                println!("{}📥 Delegation complete", prefix);
            }
        }
        AcpEvent::Usage { cost, currency, input_tokens, output_tokens, .. } => {
            println!("{}💰 Usage: {} {} (in: {}, out: {})", prefix, cost, currency, input_tokens, output_tokens);
        }
        AcpEvent::AgentEnd { status, .. } => {
            println!("{}✨ Agent finished: {:?}", prefix, status);
        }
        AcpEvent::Error { error, .. } => {
            println!("{}❌ Error: {}", prefix, error);
        }
        _ => {}
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

