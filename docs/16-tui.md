# TUI 终端交互界面

> Claude Code 风格的终端 UI——流式对话、Tool 可视化、Interrupt 审批、多会话管理

## 1. 设计动机

框架提供了完整的 Agent 运行时（AgentLoop、Tool、Interrupt、Tracing），但缺少一个开箱即用的终端交互界面。参考 Claude Code 的交互体验，提供：

- **流式文本渲染**——LLM 输出逐字符实时显示，Markdown 实时渲染
- **Tool 调用可视化**——展示 tool 名称、参数、执行进度、结果，折叠/展开
- **Interrupt 交互式处理**——中断时在终端直接展示审批提示，用户 approve/reject/输入
- **多会话管理**——通过 ThreadId 切换历史会话
- **Token 用量展示**——实时显示 prompt / completion tokens
- **命令系统**——`/help`、`/compact`、`/clear`、`/threads`、`/model` 等斜杠命令

## 2. 技术选型

| 组件 | 选型 | 理由 |
|------|------|------|
| TUI 框架 | [ratatui](https://ratatui.rs) | Rust 生态最活跃的 TUI 库，即时模式渲染，无强制异步运行时 |
| 终端后端 | crossterm | 跨平台（Windows/macOS/Linux），无需 ncurses |
| Markdown 渲染 | 自实现 / termimad | 代码块高亮 + 内联格式 |
| 语法高亮 | syntect | 代码块 syntax highlighting |
| 输入编辑 | tui-textarea 或自实现 | 多行输入、历史、Emacs/Vi key bindings |
| 异步桥接 | tokio::sync::mpsc | AgentLoop stream → TUI 事件通道 |

### Feature Flag

```toml
[features]
tui = ["dep:ratatui", "dep:crossterm", "dep:syntect", "native"]
```

TUI 仅适用于 native 平台，不编译到 WASM。

## 3. 架构

```
┌─────────────────────────────────────────────────────┐
│                    Terminal (crossterm)              │
│  ┌───────────────────────────────────────────────┐  │
│  │              Ratatui Renderer                 │  │
│  │  ┌─────────────────────────────────────────┐  │  │
│  │  │         Message Scroll View             │  │  │
│  │  │  ┌──────────────────────────────────┐   │  │  │
│  │  │  │ User: "deploy to staging"        │   │  │  │
│  │  │  ├──────────────────────────────────┤   │  │  │
│  │  │  │ Assistant:                       │   │  │  │
│  │  │  │  I'll deploy to staging...       │   │  │  │
│  │  │  │  ┌─ bash ──────────────────────┐ │   │  │  │
│  │  │  │  │ $ kubectl apply -f deploy/  │ │   │  │  │
│  │  │  │  │ deployment.apps/web applied │ │   │  │  │
│  │  │  │  └─────────────── 2.3s ── ✓ ──┘ │   │  │  │
│  │  │  │  ┌─ ⚠ INTERRUPT ──────────────┐ │   │  │  │
│  │  │  │  │ Payment $500 needs approval │ │   │  │  │
│  │  │  │  │ [Y]es  [N]o  [E]dit amount │ │   │  │  │
│  │  │  │  └─────────────────────────────┘ │   │  │  │
│  │  │  └──────────────────────────────────┘   │  │  │
│  │  └─────────────────────────────────────────┘  │  │
│  │  ┌─────────────────────────────────────────┐  │  │
│  │  │  Status: gpt-4o │ ▸ 1,234 tokens │ T3  │  │  │
│  │  └─────────────────────────────────────────┘  │  │
│  │  ┌─────────────────────────────────────────┐  │  │
│  │  │  > _                                    │  │  │
│  │  └─────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘

        ┌──────────────┐         ┌──────────────────┐
        │   TuiApp     │◄═══════►│  BuiltAgent<M,S> │
        │              │  mpsc   │  + AgentLoop      │
        │  handle_key()│         │  + ToolRegistry   │
        │  render()    │         │  + ContextStore   │
        │  dispatch()  │         └──────────────────┘
        └──────────────┘
```

### 线程模型

```
Main Thread (TUI)                 Async Task (Agent)
─────────────────                 ──────────────────
Terminal::new()                   
loop {                            
  poll crossterm events           
  ├─ Key(Enter) ──────────────►  agent.chat(input)
  │                               stream! {
  │                                 yield AgentEvent::TextDelta(...)
  │  ◄── mpsc::Receiver ─────────  tx.send(TuiEvent::Agent(event))
  │  render(event)                }
  │                               
  ├─ Key('y') on interrupt ───►  resume(approve)
  │                               continue stream...
  │                               
  ├─ Key('/') → command mode      
  ├─ Ctrl+C → graceful exit      
  └─ tick (16ms) → re-render     
}                                 
```

## 4. 核心类型

```rust
/// TUI 事件——统一 terminal 输入事件和 agent 异步事件
enum TuiEvent {
    /// 终端按键/鼠标/窗口事件
    Terminal(crossterm::event::Event),
    /// Agent stream 事件
    Agent(AgentEvent),
    /// Agent stream 结束
    AgentDone,
    /// Agent 错误
    AgentError(AgentError),
    /// 定时 tick（重绘）
    Tick,
}

/// TUI 应用状态
struct TuiApp<M: ChatModel, S: ContextStore> {
    /// Agent 实例
    agent: BuiltAgent<M, S>,
    /// 当前输入模式
    mode: InputMode,
    /// 消息历史（渲染用）
    messages: Vec<DisplayMessage>,
    /// 当前正在流式输出的文本缓冲
    streaming_text: String,
    /// 活跃的 tool call 状态
    active_tools: Vec<ToolCallDisplay>,
    /// 待处理的 interrupt
    pending_interrupt: Option<InterruptDisplay>,
    /// 当前 Thread ID（有状态模式）
    thread_id: Option<ThreadId>,
    /// 当前 Run ID
    run_id: Option<RunId>,
    /// Token 用量统计
    usage: UsageStats,
    /// 滚动偏移
    scroll_offset: u16,
    /// 输入框状态
    input: InputState,
    /// 命令历史
    command_history: Vec<String>,
    /// 状态栏信息
    status: StatusInfo,
}

/// 输入模式
enum InputMode {
    /// 正常输入（编辑区活跃）
    Normal,
    /// Agent 正在执行（输入框锁定，显示 spinner）
    Running,
    /// 等待 interrupt 响应（显示选项）
    Interrupt,
    /// 斜杠命令模式
    Command,
}

/// 用于渲染的消息结构
struct DisplayMessage {
    role: DisplayRole,
    /// Markdown 渲染后的内容（含语法高亮）
    content: RenderedContent,
    /// 嵌套的 tool 调用记录
    tool_calls: Vec<ToolCallDisplay>,
    /// 时间戳
    timestamp: chrono::DateTime<chrono::Local>,
}

enum DisplayRole {
    User,
    Assistant,
    System,
}

/// 单个 tool call 的显示状态
struct ToolCallDisplay {
    id: String,
    name: String,
    /// 参数（JSON pretty-print）
    arguments: String,
    /// 执行状态
    status: ToolDisplayStatus,
    /// 流式输出缓冲
    output_buffer: String,
    /// 执行结果
    result: Option<String>,
    /// 是否折叠
    collapsed: bool,
    /// 耗时
    elapsed: Option<Duration>,
    /// 开始时间
    started_at: Instant,
}

enum ToolDisplayStatus {
    /// 正在接收参数
    Receiving,
    /// 正在执行
    Running,
    /// 已完成
    Completed,
    /// 中断
    Interrupted,
    /// 出错
    Error(String),
}

/// Interrupt 显示状态
struct InterruptDisplay {
    interrupts: Vec<InterruptInfo>,
    /// 当前选中的选项索引
    selected: usize,
    /// 用户输入（如果 interrupt 需要自由文本）
    user_input: String,
}

/// Token 用量统计
struct UsageStats {
    total_prompt_tokens: u32,
    total_completion_tokens: u32,
    current_prompt_tokens: u32,
    current_completion_tokens: u32,
}
```

## 5. 渲染布局

### 5.1 三区布局

```
┌──────────────────────────────────────────────────┐
│                 Message Area                     │  ← 可滚动，占满剩余空间
│  (历史消息 + 当前流式输出)                         │
│                                                  │
│                                                  │
│                                                  │
├──────────────────────────────────────────────────┤
│ gpt-4o │ T:1,234/567 │ thread: t_abc │ turn 3   │  ← 状态栏（1行）
├──────────────────────────────────────────────────┤
│ > _                                              │  ← 输入区（动态高度，1-10行）
│                                                  │
└──────────────────────────────────────────────────┘
```

### 5.2 消息区渲染规则

```rust
fn render_messages(f: &mut Frame, area: Rect, app: &TuiApp) {
    // 每条消息渲染为一个 Block
    for msg in &app.messages {
        match msg.role {
            DisplayRole::User => {
                // "❯ " 前缀，浅蓝色
                render_user_message(f, msg);
            }
            DisplayRole::Assistant => {
                // Markdown 渲染：
                // - **bold** → 粗体
                // - `code` → 反色背景
                // - ```rust ... ``` → 语法高亮的代码块
                // - 列表、标题等标准 Markdown
                render_assistant_message(f, msg);

                // Tool calls 渲染为嵌套的折叠块
                for tc in &msg.tool_calls {
                    render_tool_call(f, tc);
                }
            }
            DisplayRole::System => {
                // 灰色斜体
                render_system_message(f, msg);
            }
        }
    }

    // 正在流式输出的文本
    if !app.streaming_text.is_empty() {
        render_streaming_text(f, &app.streaming_text);
    }

    // 活跃的 tool calls
    for tc in &app.active_tools {
        render_active_tool(f, tc);
    }

    // Interrupt 提示
    if let Some(interrupt) = &app.pending_interrupt {
        render_interrupt_prompt(f, interrupt);
    }
}
```

### 5.3 Tool Call 渲染样式

```
┌─ bash ───────────────────────────────── 2.3s ── ✓ ─┐
│ $ kubectl apply -f deploy/staging.yaml              │
│ deployment.apps/web-staging configured              │
│ service/web-staging unchanged                       │
└─────────────────────────────────────────────────────┘

┌─ fs (read) ──────────────────────────── 0.1s ── ✓ ─┐
│ ▸ src/main.rs (click to expand)                     │  ← 长结果默认折叠
└─────────────────────────────────────────────────────┘

┌─ web_search ─────────────────────────── ◐ running ─┐
│ Searching for "rust tui frameworks"...              │  ← 流式 Delta 输出
└─────────────────────────────────────────────────────┘
```

样式定义：

```rust
fn render_tool_call(f: &mut Frame, area: Rect, tc: &ToolCallDisplay) {
    let border_style = match tc.status {
        ToolDisplayStatus::Running => Style::default().fg(Color::Yellow),
        ToolDisplayStatus::Completed => Style::default().fg(Color::Green),
        ToolDisplayStatus::Error(_) => Style::default().fg(Color::Red),
        ToolDisplayStatus::Interrupted => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::DarkGray),
    };

    let status_indicator = match tc.status {
        ToolDisplayStatus::Running => "◐ running",
        ToolDisplayStatus::Completed => "✓",
        ToolDisplayStatus::Error(_) => "✗",
        ToolDisplayStatus::Interrupted => "⚠ interrupted",
        ToolDisplayStatus::Receiving => "… args",
    };

    let elapsed_str = tc.elapsed
        .map(|d| format!("{:.1}s", d.as_secs_f64()))
        .unwrap_or_default();

    let title = format!(" {} ", tc.name);
    let right_title = format!(" {} ── {} ", elapsed_str, status_indicator);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
        .title_alignment(Alignment::Left)
        .title_on_bottom(right_title);

    // 内容
    let content = if tc.collapsed {
        format!("▸ {} (press Enter to expand)", tc.name)
    } else {
        let mut lines = Vec::new();
        if !tc.output_buffer.is_empty() {
            lines.push(tc.output_buffer.clone());
        }
        if let Some(result) = &tc.result {
            lines.push(result.clone());
        }
        lines.join("\n")
    };

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}
```

### 5.4 Interrupt 渲染样式

```
┌─ ⚠ INTERRUPT: human_approval ───────────────────────┐
│                                                       │
│  Payment of $500.00 requires approval                 │
│                                                       │
│  Tool: process_payment                                │
│  Amount: $500.00                                      │
│  Description: Monthly subscription                    │
│                                                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │  [Y] Approve    [N] Reject    [E] Edit amount   │  │
│  └─────────────────────────────────────────────────┘  │
│                                                       │
└───────────────────────────────────────────────────────┘
```

```rust
fn render_interrupt_prompt(f: &mut Frame, area: Rect, interrupt: &InterruptDisplay) {
    for (i, info) in interrupt.interrupts.iter().enumerate() {
        let is_selected = i == interrupt.selected;

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .title(format!(" ⚠ INTERRUPT: {} ", info.kind));

        // 根据 kind 动态生成操作选项
        let options = match info.kind.as_str() {
            "human_approval" => vec![
                ("[Y] Approve", 'y'),
                ("[N] Reject", 'n'),
            ],
            "payment_confirm" => vec![
                ("[Y] Approve", 'y'),
                ("[N] Reject", 'n'),
                ("[E] Edit amount", 'e'),
            ],
            _ => vec![
                ("[Y] Continue", 'y'),
                ("[N] Cancel", 'n'),
                ("[I] Input", 'i'),
            ],
        };

        // 渲染 data 字段的内容
        let data_lines = render_json_pretty(&info.data);

        // 渲染选项栏
        let option_line: Vec<Span> = options.iter().map(|(label, _)| {
            Span::styled(*label, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        }).collect();

        // ... 组合渲染
    }
}
```

## 6. 事件循环

```rust
impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    pub async fn run(mut self) -> Result<(), Box<dyn std::error::Error>> {
        // 初始化终端
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // 事件通道
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TuiEvent>();

        // 终端事件轮询任务
        let tx_term = tx.clone();
        let _input_handle = tokio::spawn(async move {
            loop {
                if crossterm::event::poll(Duration::from_millis(16)).unwrap() {
                    if let Ok(event) = crossterm::event::read() {
                        tx_term.send(TuiEvent::Terminal(event)).ok();
                    }
                }
                tx_term.send(TuiEvent::Tick).ok();
            }
        });

        // 主循环
        loop {
            // 渲染
            terminal.draw(|f| self.render(f))?;

            // 等待事件
            if let Some(event) = rx.recv().await {
                match event {
                    TuiEvent::Terminal(crossterm::event::Event::Key(key)) => {
                        if self.handle_key(key, &tx).await? {
                            break; // 退出
                        }
                    }
                    TuiEvent::Agent(agent_event) => {
                        self.handle_agent_event(agent_event);
                    }
                    TuiEvent::AgentDone => {
                        self.on_agent_done();
                    }
                    TuiEvent::AgentError(e) => {
                        self.on_agent_error(e);
                    }
                    TuiEvent::Tick => {
                        // 更新 spinner 动画帧等
                        self.tick();
                    }
                    _ => {}
                }
            }
        }

        // 清理终端
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen, DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }
}
```

## 7. AgentEvent 处理

将 AgentEvent stream 映射为 TUI 状态变更：

```rust
impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::RunStart { run_id, thread_id, .. } => {
                self.run_id = Some(run_id);
                self.thread_id = Some(thread_id);
                self.status.state = "running";
            }

            AgentEvent::TextDelta(text) => {
                self.streaming_text.push_str(&text);
                // 增量 Markdown 解析——仅解析新增部分
            }

            AgentEvent::ToolCallStart { id, name } => {
                self.active_tools.push(ToolCallDisplay {
                    id,
                    name,
                    arguments: String::new(),
                    status: ToolDisplayStatus::Receiving,
                    output_buffer: String::new(),
                    result: None,
                    collapsed: false,
                    elapsed: None,
                    started_at: Instant::now(),
                });
            }

            AgentEvent::ToolCallArgumentsDelta { id, delta } => {
                if let Some(tc) = self.active_tools.iter_mut().find(|t| t.id == id) {
                    tc.arguments.push_str(&delta);
                }
            }

            AgentEvent::ToolDelta { id, delta, .. } => {
                if let Some(tc) = self.active_tools.iter_mut().find(|t| t.id == id) {
                    tc.status = ToolDisplayStatus::Running;
                    tc.output_buffer.push_str(&delta);
                }
            }

            AgentEvent::ToolResult { id, result, .. } => {
                if let Some(tc) = self.active_tools.iter_mut().find(|t| t.id == id) {
                    tc.status = ToolDisplayStatus::Completed;
                    tc.result = Some(result);
                    tc.elapsed = Some(tc.started_at.elapsed());
                    // 长结果自动折叠
                    if tc.result.as_ref().map(|r| r.len() > 500).unwrap_or(false) {
                        tc.collapsed = true;
                    }
                }
            }

            AgentEvent::Interrupt { interrupts } => {
                self.mode = InputMode::Interrupt;
                self.pending_interrupt = Some(InterruptDisplay {
                    interrupts,
                    selected: 0,
                    user_input: String::new(),
                });
                // 标记所有未完成的 tool 为 Interrupted
                for tc in &mut self.active_tools {
                    if matches!(tc.status, ToolDisplayStatus::Running | ToolDisplayStatus::Receiving) {
                        tc.status = ToolDisplayStatus::Interrupted;
                    }
                }
            }

            AgentEvent::TurnStart { turn } => {
                // 将当前流式文本 + tool calls 固化为 DisplayMessage
                self.finalize_current_message();
                self.status.turn = turn;
            }

            AgentEvent::Usage { prompt_tokens, completion_tokens } => {
                self.usage.current_prompt_tokens = prompt_tokens;
                self.usage.current_completion_tokens = completion_tokens;
                self.usage.total_prompt_tokens += prompt_tokens;
                self.usage.total_completion_tokens += completion_tokens;
            }

            AgentEvent::Error(e) => {
                self.status.last_error = Some(format!("{e}"));
            }

            AgentEvent::Done => {
                self.finalize_current_message();
                self.mode = InputMode::Normal;
                self.status.state = "idle";
            }
        }
    }

    /// 将当前流式内容固化为一条完整的 DisplayMessage
    fn finalize_current_message(&mut self) {
        if !self.streaming_text.is_empty() || !self.active_tools.is_empty() {
            self.messages.push(DisplayMessage {
                role: DisplayRole::Assistant,
                content: render_markdown(&self.streaming_text),
                tool_calls: std::mem::take(&mut self.active_tools),
                timestamp: chrono::Local::now(),
            });
            self.streaming_text.clear();
        }
    }
}
```

## 8. 用户输入处理

### 8.1 按键映射

```rust
impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    async fn handle_key(
        &mut self,
        key: KeyEvent,
        tx: &mpsc::UnboundedSender<TuiEvent>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        // 全局快捷键
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.mode == InputMode::Running {
                    // Ctrl+C 在运行中 → 取消当前 run（首次温和，二次强制）
                    self.cancel_run();
                    return Ok(false);
                }
                return Ok(true); // 退出
            }
            _ => {}
        }

        match self.mode {
            InputMode::Normal => self.handle_normal_key(key, tx).await,
            InputMode::Running => Ok(false), // 运行中忽略大部分按键
            InputMode::Interrupt => self.handle_interrupt_key(key, tx).await,
            InputMode::Command => self.handle_command_key(key, tx).await,
        }
    }

    async fn handle_normal_key(
        &mut self,
        key: KeyEvent,
        tx: &mpsc::UnboundedSender<TuiEvent>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match key.code {
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                // Shift+Enter → 换行
                self.input.insert_newline();
            }
            KeyCode::Enter => {
                let input = self.input.take_text();
                if input.is_empty() { return Ok(false); }

                if input.starts_with('/') {
                    self.handle_command(&input).await?;
                } else {
                    self.send_message(input, tx).await?;
                }
            }
            KeyCode::Up => {
                // 输入为空时 → 历史命令
                if self.input.is_empty() {
                    self.input.set_from_history(&self.command_history);
                } else {
                    self.scroll_offset = self.scroll_offset.saturating_add(3);
                }
            }
            KeyCode::Down => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
            }
            KeyCode::Char(c) => {
                self.input.insert_char(c);
            }
            KeyCode::Backspace => {
                self.input.delete_char();
            }
            KeyCode::Tab => {
                // Tab → 折叠/展开最近的 tool call
                self.toggle_last_tool_collapse();
            }
            _ => {}
        }
        Ok(false)
    }
}
```

### 8.2 发送消息并启动 Agent

```rust
impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    async fn send_message(
        &mut self,
        input: String,
        tx: &mpsc::UnboundedSender<TuiEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 添加用户消息到显示
        self.messages.push(DisplayMessage {
            role: DisplayRole::User,
            content: render_markdown(&input),
            tool_calls: vec![],
            timestamp: chrono::Local::now(),
        });

        self.mode = InputMode::Running;
        self.command_history.push(input.clone());

        // 启动 Agent stream
        let tx = tx.clone();
        let stream = if let Some(thread_id) = &self.thread_id {
            self.agent.chat_in_thread(thread_id, input).await?
        } else {
            self.agent.chat(input).await?
        };

        // 异步消费 stream，通过 mpsc 发送到 TUI 主循环
        tokio::spawn(async move {
            pin_mut!(stream);
            while let Some(event) = stream.next().await {
                if tx.send(TuiEvent::Agent(event)).is_err() {
                    break;
                }
            }
            tx.send(TuiEvent::AgentDone).ok();
        });

        Ok(())
    }
}
```

### 8.3 Interrupt 交互处理

```rust
impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    async fn handle_interrupt_key(
        &mut self,
        key: KeyEvent,
        tx: &mpsc::UnboundedSender<TuiEvent>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let interrupt = match &mut self.pending_interrupt {
            Some(i) => i,
            None => return Ok(false),
        };

        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Approve 所有 interrupts
                let payloads: Vec<ResumePayload> = interrupt.interrupts.iter()
                    .map(|info| ResumePayload {
                        interrupt_id: info.interrupt_id.clone(),
                        result: serde_json::json!({ "approved": true }),
                    })
                    .collect();
                self.resume_agent(payloads, tx).await?;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                // Reject 所有 interrupts
                let payloads: Vec<ResumePayload> = interrupt.interrupts.iter()
                    .map(|info| ResumePayload {
                        interrupt_id: info.interrupt_id.clone(),
                        result: serde_json::json!({ "approved": false, "reason": "User rejected" }),
                    })
                    .collect();
                self.resume_agent(payloads, tx).await?;
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                // 进入编辑模式——让用户输入自定义值
                self.mode = InputMode::Normal;
                self.status.hint = Some("Enter value, then press Enter to submit".into());
                // 保留 pending_interrupt，下次 Enter 时提交
            }
            KeyCode::Tab => {
                // 切换选中的 interrupt（多个时）
                interrupt.selected = (interrupt.selected + 1) % interrupt.interrupts.len();
            }
            _ => {}
        }
        Ok(false)
    }

    async fn resume_agent(
        &mut self,
        payloads: Vec<ResumePayload>,
        tx: &mpsc::UnboundedSender<TuiEvent>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.pending_interrupt = None;
        self.mode = InputMode::Running;

        let thread_id = self.thread_id.as_ref().unwrap().clone();
        let run_id = self.run_id.as_ref().unwrap().clone();

        let stream = self.agent
            .resume_run(&thread_id, &run_id, payloads)
            .await?;

        let tx = tx.clone();
        tokio::spawn(async move {
            pin_mut!(stream);
            while let Some(event) = stream.next().await {
                if tx.send(TuiEvent::Agent(event)).is_err() {
                    break;
                }
            }
            tx.send(TuiEvent::AgentDone).ok();
        });

        Ok(())
    }
}
```

## 9. 斜杠命令系统

```rust
/// 支持的斜杠命令
enum SlashCommand {
    Help,                             // /help — 显示帮助
    Clear,                            // /clear — 清空当前会话显示
    Compact,                          // /compact — 压缩上下文（摘要历史）
    Model(String),                    // /model gpt-4o — 切换模型
    Threads,                          // /threads — 列出所有 threads
    Thread(ThreadId),                 // /thread t_xxx — 切换到指定 thread
    NewThread,                        // /new — 创建新 thread
    Config(String, String),           // /config key value — 修改运行时配置
    Tools,                            // /tools — 列出已注册的 tools
    Usage,                            // /usage — 显示详细用量统计
    History,                          // /history — 显示命令历史
    Export(String),                   // /export path — 导出当前对话为 Markdown
    Debug,                            // /debug — 切换 debug 模式（显示原始事件）
}

impl<M: ChatModel, S: ContextStore> TuiApp<M, S> {
    async fn handle_command(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0];
        let arg = parts.get(1).map(|s| s.trim());

        match cmd {
            "/help" | "/h" | "/?" => {
                self.show_system_message(HELP_TEXT);
            }

            "/clear" => {
                self.messages.clear();
                self.streaming_text.clear();
                self.active_tools.clear();
            }

            "/compact" => {
                // 请求 Agent 压缩上下文
                self.show_system_message("Compacting context...");
                // 将当前历史摘要化，替换 messages（如果 ContextStore 支持）
                if let Some(thread_id) = &self.thread_id {
                    // compact_thread() 将历史消息摘要化
                    // 实现依赖 ContextStore 的具体后端
                }
            }

            "/model" => {
                if let Some(model_name) = arg {
                    self.status.model = model_name.to_string();
                    self.show_system_message(&format!("Switched to model: {model_name}"));
                } else {
                    self.show_system_message(&format!("Current model: {}", self.status.model));
                }
            }

            "/threads" => {
                if let Some(store) = self.agent.store() {
                    // 列出所有 threads
                    self.show_system_message("Available threads:\n...");
                } else {
                    self.show_system_message("No context store configured (stateless mode)");
                }
            }

            "/new" => {
                if let Some(store) = self.agent.store() {
                    let thread_id = self.agent.create_thread().await?;
                    self.thread_id = Some(thread_id.clone());
                    self.messages.clear();
                    self.show_system_message(&format!("New thread: {}", thread_id.0));
                }
            }

            "/tools" => {
                let definitions = self.agent.tool_definitions();
                let mut text = String::from("Registered tools:\n");
                for def in &definitions {
                    text.push_str(&format!("  - {} — {}\n", def.function.name, def.function.description));
                }
                self.show_system_message(&text);
            }

            "/usage" => {
                self.show_system_message(&format!(
                    "Token usage:\n  Current: {} prompt + {} completion\n  Total: {} prompt + {} completion",
                    self.usage.current_prompt_tokens,
                    self.usage.current_completion_tokens,
                    self.usage.total_prompt_tokens,
                    self.usage.total_completion_tokens,
                ));
            }

            "/export" => {
                if let Some(path) = arg {
                    let md = self.export_as_markdown();
                    tokio::fs::write(path, md).await?;
                    self.show_system_message(&format!("Exported to {path}"));
                } else {
                    self.show_system_message("Usage: /export <path>");
                }
            }

            "/debug" => {
                self.status.debug_mode = !self.status.debug_mode;
                self.show_system_message(&format!(
                    "Debug mode: {}", if self.status.debug_mode { "ON" } else { "OFF" }
                ));
            }

            _ => {
                self.show_system_message(&format!("Unknown command: {cmd}. Type /help for help."));
            }
        }
        Ok(())
    }
}
```

### 帮助文本

```rust
const HELP_TEXT: &str = r#"
Commands:
  /help, /h          Show this help
  /clear             Clear the display
  /compact           Compact context (summarize history)
  /model [name]      Show or switch model
  /threads           List all threads
  /thread <id>       Switch to thread
  /new               Create new thread
  /tools             List registered tools
  /usage             Show token usage
  /export <path>     Export conversation as Markdown
  /debug             Toggle debug mode (show raw events)
  /history           Show command history
  /config <k> <v>    Update runtime config

Shortcuts:
  Enter              Send message
  Shift+Enter        New line
  Ctrl+C             Cancel run / Exit
  Up/Down            Scroll history
  Tab                Collapse/expand tool output
  Esc                Cancel current input
"#;
```

## 10. Markdown 实时渲染

```rust
/// 流式 Markdown 渲染器——支持增量解析
struct MarkdownRenderer {
    /// 当前解析状态
    state: MdParseState,
    /// 已渲染的行
    rendered_lines: Vec<Line<'static>>,
    /// syntect 高亮器
    syntax_set: SyntaxSet,
    theme: Theme,
}

enum MdParseState {
    Normal,
    InCodeBlock { lang: String, content: String },
    InBold,
    InInlineCode,
}

impl MarkdownRenderer {
    fn push_text(&mut self, text: &str) {
        for ch in text.chars() {
            match &mut self.state {
                MdParseState::InCodeBlock { lang, content } => {
                    content.push(ch);
                    // 检测 ``` 结束
                    if content.ends_with("```") {
                        let code = content.trim_end_matches("```");
                        let highlighted = self.highlight_code(code, lang);
                        self.rendered_lines.extend(highlighted);
                        self.state = MdParseState::Normal;
                    }
                }
                MdParseState::Normal => {
                    // 检测 ```lang 开始
                    // 检测 **bold**、`inline code`
                    // 检测 # heading
                    // 检测 - list item
                    // 逐字符状态机推进
                }
                _ => {}
            }
        }
    }

    fn highlight_code(&self, code: &str, lang: &str) -> Vec<Line<'static>> {
        let syntax = self.syntax_set
            .find_syntax_by_token(lang)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, &self.theme);
        code.lines()
            .map(|line| {
                let ranges = h.highlight_line(line, &self.syntax_set).unwrap();
                let spans: Vec<Span> = ranges.iter().map(|(style, text)| {
                    Span::styled(
                        text.to_string(),
                        syntect_style_to_ratatui(*style),
                    )
                }).collect();
                Line::from(spans)
            })
            .collect()
    }
}
```

## 11. CLI 入口

```rust
/// remi-tui 二进制入口
///
/// 用法：
///   remi-tui                        # 交互式，使用环境变量配置
///   remi-tui --model gpt-4o          # 指定模型
///   remi-tui --thread t_xxx          # 恢复已有会话
///   remi-tui -c "summarize this"     # 单次执行（非交互）
///   echo "..." | remi-tui            # 管道输入
use clap::Parser;

#[derive(Parser)]
#[command(name = "remi-tui", about = "Claude Code-style AI agent TUI")]
struct Cli {
    /// Model to use (default: from REMI_MODEL or gpt-4o)
    #[arg(short, long)]
    model: Option<String>,

    /// Resume an existing thread
    #[arg(short, long)]
    thread: Option<String>,

    /// Single-shot command (non-interactive)
    #[arg(short = 'c', long = "command")]
    command: Option<String>,

    /// System prompt override
    #[arg(short, long)]
    system: Option<String>,

    /// Enable tools (comma-separated: bash,fs,vfs)
    #[arg(long, value_delimiter = ',')]
    tools: Vec<String>,

    /// Working directory for file tools
    #[arg(short = 'd', long)]
    workdir: Option<PathBuf>,

    /// Config file path
    #[arg(long, default_value = "~/.config/remi/config.toml")]
    config: PathBuf,

    /// Enable debug mode
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // 加载配置（环境变量 > 配置文件 > CLI 参数）
    let config = load_config(&cli)?;

    // 构建 Agent
    let model = OpenAIClient::from_config(&config);
    let mut builder = AgentBuilder::new()
        .model(model)
        .system(cli.system.as_deref().unwrap_or(DEFAULT_SYSTEM_PROMPT));

    // 注册 tools
    for tool_name in &cli.tools {
        match tool_name.as_str() {
            "bash" => { builder = builder.tool(BashTool::new()); }
            "fs" => {
                let root = cli.workdir.clone().unwrap_or_else(|| PathBuf::from("."));
                builder = builder.tool(FsTool::new(root).writable());
            }
            "vfs" => { builder = builder.tool(VirtualFsTool::new()); }
            _ => eprintln!("Unknown tool: {tool_name}"),
        }
    }

    // 构建
    let store = InMemoryStore::new();
    let agent = builder.context_store(store).build();

    // 单次执行模式
    if let Some(command) = cli.command {
        return run_oneshot(agent, command).await;
    }

    // 管道输入检测
    if atty::isnt(atty::Stream::Stdin) {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        return run_oneshot(agent, input).await;
    }

    // 交互式 TUI
    let thread_id = if let Some(tid) = cli.thread {
        Some(ThreadId(tid))
    } else {
        let tid = agent.create_thread().await?;
        Some(tid)
    };

    let app = TuiApp::new(agent, thread_id, cli.debug);
    app.run().await
}

/// 单次执行——非交互模式，直接打印到 stdout
async fn run_oneshot<M: ChatModel, S: ContextStore>(
    agent: BuiltAgent<M, S>,
    input: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = agent.chat(input).await?;
    pin_mut!(stream);
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta(text) => {
                print!("{text}");
                std::io::stdout().flush()?;
            }
            AgentEvent::ToolResult { name, result, .. } => {
                eprintln!("\n[tool:{name}] {result}");
            }
            AgentEvent::Error(e) => {
                eprintln!("\nError: {e}");
            }
            _ => {}
        }
    }
    println!();
    Ok(())
}

const DEFAULT_SYSTEM_PROMPT: &str = r#"You are a helpful AI assistant running in a terminal. You have access to tools for executing commands and managing files. Be concise and precise."#;
```

## 12. 配置文件

```toml
# ~/.config/remi/config.toml

[model]
provider = "openai"        # openai | anthropic | custom
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"  # 从环境变量读取
# base_url = "https://api.openai.com/v1"  # 可选覆盖

[tui]
theme = "dark"             # dark | light
show_token_usage = true
auto_collapse_tools = true       # 长结果自动折叠
max_display_lines = 200          # 单个 tool 结果最大显示行数
markdown_render = true           # 是否渲染 Markdown
code_theme = "base16-ocean.dark" # syntect 主题

[tools]
enabled = ["bash", "fs"]
bash_timeout_secs = 30
fs_root = "."
fs_writable = true
fs_max_read_bytes = 10485760     # 10MB

[context]
store = "memory"           # memory | sqlite | redis
# sqlite_path = "~/.config/remi/history.db"
max_turns = 25
```

## 13. 模块结构

```
src/
├── tui/                    # [feature: tui]
│   ├── mod.rs              # TuiApp, TuiEvent, pub fn run()
│   ├── app.rs              # TuiApp state + event handling
│   ├── render.rs           # Ratatui rendering (messages, tools, interrupts)
│   ├── markdown.rs         # Streaming Markdown parser + renderer
│   ├── input.rs            # Input state, key bindings, history
│   ├── commands.rs         # Slash command parsing + dispatch
│   └── theme.rs            # Colors, styles, Unicode symbols
├── ...

bin/
└── remi-tui.rs             # CLI 入口 (clap + main)
```

### Cargo.toml 新增

```toml
[[bin]]
name = "remi-tui"
path = "bin/remi-tui.rs"
required-features = ["tui"]

[features]
tui = [
    "dep:ratatui",
    "dep:crossterm",
    "dep:syntect",
    "dep:clap",
    "dep:atty",
    "native",
    "tool-bash",
    "tool-fs",
]

[dependencies]
ratatui = { version = "0.28", optional = true }
crossterm = { version = "0.28", optional = true }
syntect = { version = "5", optional = true, default-features = false, features = ["default-fancy"] }
clap = { version = "4", optional = true, features = ["derive"] }
atty = { version = "0.2", optional = true }
```

## 14. Roadmap 影响

Phase 6 新增：

- `34.` `src/tui/` — TUI 模块（ratatui + crossterm）
- `35.` `bin/remi-tui.rs` — CLI 入口（clap）
- `36.` TUI Markdown 流式渲染 + 代码高亮
- `37.` TUI Interrupt 交互式审批
- `38.` TUI 端到端测试（headless terminal）
