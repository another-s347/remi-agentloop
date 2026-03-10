// ── Protocol events (mirrors remi-core ProtocolEvent) ─────────────────────────

export interface ContentPart {
  type: 'text' | 'image'
  text?: string
  image_url?: string
  mime_type?: string
}

export type Content = string | ContentPart[]

export interface Message {
  role: 'user' | 'assistant' | 'tool'
  content: Content
  tool_call_id?: string
  tool_calls?: ToolCallInfo[]
}

export interface ToolCallInfo {
  id: string
  name: string
  arguments: string
}

export interface ToolDefinition {
  name: string
  description: string
  parameters: Record<string, unknown>
}

export interface AgentState {
  [key: string]: unknown
}

export interface ToolCallOutcome {
  id: string
  result: string
}

export interface InterruptInfo {
  kind: string
  message: string
  [key: string]: unknown
}

// ── ProtocolEvent discriminated union ────────────────────────────────────────

export type ProtocolEvent =
  | { type: 'run_start'; thread_id: string; run_id: string; metadata?: unknown }
  | { type: 'delta'; content: string; role?: string }
  | { type: 'thinking_start' }
  | { type: 'thinking_end'; content: string }
  | { type: 'tool_call_start'; id: string; name: string }
  | { type: 'tool_call_delta'; id: string; arguments_delta: string }
  | { type: 'tool_delta'; id: string; name: string; delta: string }
  | { type: 'tool_result'; id: string; name: string; result: string }
  | { type: 'interrupt'; interrupts: InterruptInfo[] }
  | { type: 'turn_start'; turn: number }
  | { type: 'usage'; prompt_tokens: number; completion_tokens: number }
  | { type: 'error'; message: string; code?: string }
  | { type: 'done' }
  | { type: 'cancelled' }
  | { type: 'need_tool_execution'; state: AgentState; tool_calls: ToolCallInfo[]; completed_results: ToolCallOutcome[] }
  | { type: 'custom'; event_type: string; extra?: unknown }

// ── LoopInput (POST /chat body) ───────────────────────────────────────────────

export type LoopInput =
  | {
    type: 'start'
    content: Content
    history: Message[]
    extra_tools: ToolDefinition[]
    model?: string
    temperature?: number
    max_tokens?: number
    metadata?: unknown
  }
  | {
    type: 'resume'
    state: AgentState
    results: ToolCallOutcome[]
  }
  | {
    type: 'cancel'
    state: AgentState
  }

// ── Build status events (from GET /status) ────────────────────────────────────

export type BuildStatusEvent =
  | { type: 'build_start' }
  | { type: 'build_ok'; crate_name: string }
  | { type: 'build_error'; message: string }
  | { type: 'agent_reloaded'; crate_name: string }
  | { type: 'ping' }
