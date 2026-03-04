import { useCallback, useRef, useState } from 'react'
import type { ProtocolEvent } from '../types'
import { useStore } from '../store'

function makeId(): string {
  return Math.random().toString(36).slice(2, 10) + Date.now().toString(36)
}

// No new SSE event for this many ms → abort and show error
const INACTIVITY_TIMEOUT_MS = 60_000

export function useChat(sessionId: string | null) {
  const { dispatch } = useStore()
  const [isSending, setIsSending] = useState(false)
  const abortRef = useRef<AbortController | null>(null)

  const send = useCallback(
    async (
      content: string,
      history: import('../types').Message[],
      metadata?: Record<string, unknown>,
    ) => {
      if (!sessionId || isSending) return

      setIsSending(true)

      // Add user message to display
      dispatch({ type: 'ADD_USER_MESSAGE', sessionId, content })

      // Prepare assistant message slot
      const msgId = makeId()
      dispatch({ type: 'START_ASSISTANT_STREAMING', sessionId, msgId })

      const controller = new AbortController()
      abortRef.current = controller

      let accumulatedContent = ''
      let lastUsage: { prompt_tokens: number; completion_tokens: number } | undefined
      let lastActivityAt = Date.now()

      // Inactivity watchdog — aborts if no SSE event arrives within the timeout
      const watchdog = setInterval(() => {
        if (Date.now() - lastActivityAt > INACTIVITY_TIMEOUT_MS) {
          controller.abort()
        }
      }, 5_000)

      try {
        const response = await fetch('/chat', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            type: 'start',
            content,
            history,
            extra_tools: [],
            ...(metadata && Object.keys(metadata).length > 0 ? { metadata } : {}),
          }),
          signal: controller.signal,
        })

        if (!response.ok) {
          throw new Error(`HTTP ${response.status}: ${response.statusText}`)
        }

        const reader = response.body?.getReader()
        if (!reader) throw new Error('No response body')

        const decoder = new TextDecoder()
        let buffer = ''

        // SSE parser
        while (true) {
          const { done, value } = await reader.read()
          if (done) break

          buffer += decoder.decode(value, { stream: true })
          const lines = buffer.split('\n')
          buffer = lines.pop() ?? ''

          let eventType = ''
          let dataLine = ''

          for (const line of lines) {
            if (line.startsWith('event: ')) {
              eventType = line.slice(7).trim()
            } else if (line.startsWith('data: ')) {
              dataLine = line.slice(6).trim()
            } else if (line === '' && dataLine) {
              lastActivityAt = Date.now()
              // Dispatch the parsed event
              try {
                const parsed = JSON.parse(dataLine) as ProtocolEvent

                dispatch({ type: 'PUSH_EVENT', sessionId, msgId, event: parsed })

                switch (parsed.type) {
                  case 'delta':
                    accumulatedContent += parsed.content
                    dispatch({ type: 'APPEND_DELTA', sessionId, msgId, delta: parsed.content })
                    break
                  case 'thinking_start':
                    dispatch({ type: 'THINKING_START', sessionId, msgId })
                    break
                  case 'thinking_end':
                    dispatch({ type: 'THINKING_END', sessionId, msgId, content: parsed.content })
                    break
                  case 'tool_call_start':
                    dispatch({ type: 'TOOL_CALL_START', sessionId, msgId, id: parsed.id, name: parsed.name })
                    break
                  case 'tool_call_delta':
                    dispatch({ type: 'TOOL_CALL_DELTA', sessionId, msgId, id: parsed.id, delta: parsed.arguments_delta })
                    break
                  case 'tool_result':
                    dispatch({ type: 'TOOL_RESULT', sessionId, msgId, id: parsed.id, result: parsed.result })
                    break
                  case 'turn_start':
                    dispatch({ type: 'SET_TURN', sessionId, msgId, turn: parsed.turn })
                    break
                  case 'usage':
                    lastUsage = { prompt_tokens: parsed.prompt_tokens, completion_tokens: parsed.completion_tokens }
                    break
                  case 'error':
                    dispatch({ type: 'SET_ASSISTANT_ERROR', sessionId, msgId, error: parsed.message })
                    setIsSending(false)
                    return
                  case 'done':
                    // Intermediate done between turns — keep the SSE stream open
                    break
                  case 'cancelled':
                    dispatch({ type: 'FINALIZE_ASSISTANT', sessionId, msgId, usage: lastUsage })
                    dispatch({
                      type: 'COMMIT_HISTORY',
                      sessionId,
                      userContent: content,
                      assistantContent: accumulatedContent,
                    })
                    setIsSending(false)
                    return
                  default:
                    break
                }
              } catch {
                // ignore parse errors
              }

              eventType = ''
              dataLine = ''
            }
          }
        }
        // Stream ended without a done/error event — finalize gracefully
        dispatch({ type: 'FINALIZE_ASSISTANT', sessionId, msgId, usage: lastUsage })
        if (accumulatedContent) {
          dispatch({ type: 'COMMIT_HISTORY', sessionId, userContent: content, assistantContent: accumulatedContent })
        }
      } catch (err: unknown) {
        if (err instanceof Error && err.name === 'AbortError') {
          // Check if aborted due to inactivity
          if (Date.now() - lastActivityAt > INACTIVITY_TIMEOUT_MS - 5_000) {
            dispatch({ type: 'SET_ASSISTANT_ERROR', sessionId, msgId, error: 'No response from agent (timeout). Check your LLM settings.' })
          } else {
            dispatch({ type: 'FINALIZE_ASSISTANT', sessionId, msgId })
          }
        } else {
          const msg = err instanceof Error ? err.message : String(err)
          dispatch({ type: 'SET_ASSISTANT_ERROR', sessionId, msgId, error: msg })
        }
      } finally {
        clearInterval(watchdog)
        setIsSending(false)
        abortRef.current = null
      }
    },
    [sessionId, isSending, dispatch]
  )

  const cancel = useCallback(() => {
    abortRef.current?.abort()
  }, [])

  return { send, cancel, isSending }
}
