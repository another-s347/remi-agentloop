import React, { createContext, useContext, useReducer, useEffect } from 'react'
import type { Message, ProtocolEvent } from './types'

// ── Session model ─────────────────────────────────────────────────────────────

export interface ToolCallState {
  id: string
  name: string
  argumentsDelta: string
  result?: string
}

export interface ChatMessage {
  id: string
  role: 'user' | 'assistant'
  content: string
  /** streaming state — undefined means complete */
  streaming?: boolean
  /** true while ThinkingStart received but ThinkingEnd not yet */
  isThinking?: boolean
  /** full reasoning/thinking text, set when ThinkingEnd arrives */
  thinkingContent?: string
  /** tool calls attached to this assistant turn */
  toolCalls: ToolCallState[]
  /** protocol events received during this message (for timeline) */
  events: ProtocolEvent[]
  usage?: { prompt_tokens: number; completion_tokens: number }
  error?: string
  turnIndex?: number
}

export interface Session {
  id: string
  name: string
  /** Full raw Messages for history (sent to /chat on next turn) */
  history: Message[]
  /** Display messages (rendered in UI) */
  messages: ChatMessage[]
  createdAt: number
  updatedAt: number
}

// ── State ─────────────────────────────────────────────────────────────────────

export interface StoreState {
  sessions: Session[]
  activeSessionId: string | null
}

const STORAGE_KEY = 'remi_sessions'

function loadFromStorage(): StoreState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) return JSON.parse(raw)
  } catch { }
  return { sessions: [], activeSessionId: null }
}

function saveToStorage(state: StoreState) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state))
  } catch { }
}

// ── Actions ───────────────────────────────────────────────────────────────────

export type Action =
  | { type: 'CREATE_SESSION' }
  | { type: 'DELETE_SESSION'; sessionId: string }
  | { type: 'RENAME_SESSION'; sessionId: string; name: string }
  | { type: 'SELECT_SESSION'; sessionId: string }
  | { type: 'ADD_USER_MESSAGE'; sessionId: string; content: string }
  | { type: 'START_ASSISTANT_STREAMING'; sessionId: string; msgId: string }
  | { type: 'APPEND_DELTA'; sessionId: string; msgId: string; delta: string }
  | { type: 'TOOL_CALL_START'; sessionId: string; msgId: string; id: string; name: string }
  | { type: 'TOOL_CALL_DELTA'; sessionId: string; msgId: string; id: string; delta: string }
  | { type: 'TOOL_RESULT'; sessionId: string; msgId: string; id: string; result: string }
  | { type: 'PUSH_EVENT'; sessionId: string; msgId: string; event: ProtocolEvent }
  | { type: 'FINALIZE_ASSISTANT'; sessionId: string; msgId: string; usage?: { prompt_tokens: number; completion_tokens: number } }
  | { type: 'SET_ASSISTANT_ERROR'; sessionId: string; msgId: string; error: string }
  | { type: 'COMMIT_HISTORY'; sessionId: string; userContent: string; assistantContent: string }
  | { type: 'SET_TURN'; sessionId: string; msgId: string; turn: number }
  | { type: 'THINKING_START'; sessionId: string; msgId: string }
  | { type: 'THINKING_END'; sessionId: string; msgId: string; content: string }

// ── Pure helpers ──────────────────────────────────────────────────────────────

function makeId(): string {
  return Math.random().toString(36).slice(2, 10) + Date.now().toString(36)
}

function updateSession(state: StoreState, sessionId: string, fn: (s: Session) => Session): StoreState {
  return {
    ...state,
    sessions: state.sessions.map(s => (s.id === sessionId ? fn(s) : s)),
  }
}

function updateMessage(session: Session, msgId: string, fn: (m: ChatMessage) => ChatMessage): Session {
  return {
    ...session,
    messages: session.messages.map(m => (m.id === msgId ? fn(m) : m)),
    updatedAt: Date.now(),
  }
}

// ── Reducer ───────────────────────────────────────────────────────────────────

function reducer(state: StoreState, action: Action): StoreState {
  switch (action.type) {
    case 'CREATE_SESSION': {
      const id = makeId()
      const session: Session = {
        id,
        name: 'New chat',
        history: [],
        messages: [],
        createdAt: Date.now(),
        updatedAt: Date.now(),
      }
      return { sessions: [session, ...state.sessions], activeSessionId: id }
    }

    case 'DELETE_SESSION': {
      const sessions = state.sessions.filter(s => s.id !== action.sessionId)
      const activeSessionId =
        state.activeSessionId === action.sessionId
          ? sessions[0]?.id ?? null
          : state.activeSessionId
      return { sessions, activeSessionId }
    }

    case 'RENAME_SESSION':
      return updateSession(state, action.sessionId, s => ({ ...s, name: action.name }))

    case 'SELECT_SESSION':
      return { ...state, activeSessionId: action.sessionId }

    case 'ADD_USER_MESSAGE': {
      return updateSession(state, action.sessionId, s => ({
        ...s,
        updatedAt: Date.now(),
        messages: [
          ...s.messages,
          {
            id: makeId(),
            role: 'user',
            content: action.content,
            toolCalls: [],
            events: [],
          },
        ],
      }))
    }

    case 'START_ASSISTANT_STREAMING': {
      return updateSession(state, action.sessionId, s => ({
        ...s,
        updatedAt: Date.now(),
        messages: [
          ...s.messages,
          {
            id: action.msgId,
            role: 'assistant',
            content: '',
            streaming: true,
            toolCalls: [],
            events: [],
          },
        ],
      }))
    }

    case 'APPEND_DELTA':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, content: m.content + action.delta }))
      )

    case 'TOOL_CALL_START':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({
          ...m,
          toolCalls: [...m.toolCalls, { id: action.id, name: action.name, argumentsDelta: '' }],
        }))
      )

    case 'TOOL_CALL_DELTA':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({
          ...m,
          toolCalls: m.toolCalls.map(tc =>
            tc.id === action.id ? { ...tc, argumentsDelta: tc.argumentsDelta + action.delta } : tc
          ),
        }))
      )

    case 'TOOL_RESULT':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({
          ...m,
          toolCalls: m.toolCalls.map(tc =>
            tc.id === action.id ? { ...tc, result: action.result } : tc
          ),
        }))
      )

    case 'PUSH_EVENT':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, events: [...m.events, action.event] }))
      )

    case 'SET_TURN':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, turnIndex: action.turn }))
      )

    case 'THINKING_START':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, isThinking: true }))
      )

    case 'THINKING_END':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, isThinking: false, thinkingContent: action.content }))
      )

    case 'FINALIZE_ASSISTANT':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, streaming: false, usage: action.usage ?? m.usage }))
      )

    case 'SET_ASSISTANT_ERROR':
      return updateSession(state, action.sessionId, s =>
        updateMessage(s, action.msgId, m => ({ ...m, streaming: false, error: action.error }))
      )

    case 'COMMIT_HISTORY':
      return updateSession(state, action.sessionId, s => ({
        ...s,
        history: [
          ...s.history,
          { role: 'user', content: action.userContent },
          { role: 'assistant', content: action.assistantContent },
        ],
      }))

    default:
      return state
  }
}

// ── Context ───────────────────────────────────────────────────────────────────

interface StoreContextValue {
  state: StoreState
  dispatch: React.Dispatch<Action>
  activeSession: Session | null
}

const StoreContext = createContext<StoreContextValue | null>(null)

export function SessionProvider({ children }: { children: React.ReactNode }) {
  const [state, dispatch] = useReducer(reducer, undefined, loadFromStorage)

  useEffect(() => {
    saveToStorage(state)
  }, [state])

  const activeSession = state.sessions.find(s => s.id === state.activeSessionId) ?? null

  return <StoreContext.Provider value={{ state, dispatch, activeSession }}>{children}</StoreContext.Provider>
}

export function useStore(): StoreContextValue {
  const ctx = useContext(StoreContext)
  if (!ctx) throw new Error('useStore must be inside SessionProvider')
  return ctx
}
