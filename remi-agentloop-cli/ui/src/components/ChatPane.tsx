import { useCallback, useEffect, useRef, useState } from 'react'
import { useStore } from '../store'
import { useChat } from '../hooks/useChat'
import { readSettings } from '../hooks/useSettings'
import { MessageBubble } from './MessageBubble'

export function ChatPane() {
  const { activeSession, dispatch } = useStore()
  const { send, cancel, isSending } = useChat(activeSession?.id ?? null)
  const [input, setInput] = useState('')
  const bottomRef = useRef<HTMLDivElement>(null)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [activeSession?.messages])

  // Auto-resize textarea
  function handleInput(e: React.ChangeEvent<HTMLTextAreaElement>) {
    setInput(e.target.value)
    const el = e.target
    el.style.height = 'auto'
    el.style.height = Math.min(el.scrollHeight, 200) + 'px'
  }

  const handleSend = useCallback(() => {
    const trimmed = input.trim()
    if (!trimmed || isSending || !activeSession) return
    // Auto-name session from first message
    if (activeSession.messages.length === 0) {
      const name = trimmed.slice(0, 40) + (trimmed.length > 40 ? '…' : '')
      dispatch({ type: 'RENAME_SESSION', sessionId: activeSession.id, name })
    }
    setInput('')
    if (textareaRef.current) {
      textareaRef.current.style.height = 'auto'
    }
    const { apiKey, baseUrl, model } = readSettings()
    const metadata = apiKey ? { api_key: apiKey, base_url: baseUrl, model } : undefined
    send(trimmed, activeSession.history, metadata)
  }, [input, isSending, activeSession, dispatch, send])

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    }
  }

  if (!activeSession) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center text-surface-600 gap-4">
        <div className="w-12 h-12 rounded-full bg-surface-800 flex items-center justify-center text-brand-400 text-xl font-bold">
          R
        </div>
        <p className="text-sm">Select a session or create a new one</p>
      </div>
    )
  }

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* Chat header */}
      <div className="flex items-center px-6 py-3 border-b border-surface-800 bg-surface-900/50 backdrop-blur-sm">
        <h2 className="text-sm font-medium text-surface-200 truncate">{activeSession.name}</h2>
        {activeSession.history.length > 0 && (
          <span className="ml-2 text-xs text-surface-600">
            {Math.floor(activeSession.history.length / 2)} turn{activeSession.history.length / 2 !== 1 ? 's' : ''}
          </span>
        )}
      </div>

      {/* Messages area */}
      <div className="flex-1 overflow-y-auto px-4 py-6">
        <div className="max-w-3xl mx-auto">
          {activeSession.messages.length === 0 && (
            <div className="flex flex-col items-center justify-center py-16 text-surface-600 gap-3">
              <svg className="w-8 h-8 opacity-40" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.5}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" />
              </svg>
              <p className="text-sm">Start a conversation</p>
            </div>
          )}

          {activeSession.messages.map((msg, i) => {
            // Find the last user message before this assistant message for retry
            let retryFn: (() => void) | undefined = undefined
            if (
              msg.role === 'assistant' &&
              msg.error &&
              !isSending &&
              i === activeSession.messages.length - 1
            ) {
              const prevUser = [...activeSession.messages].slice(0, i).reverse().find(m => m.role === 'user')
              if (prevUser) {
                const hist = activeSession.history
                const lastUserIdx = [...hist].map((h, idx) => ({ h, idx })).reverse()
                  .find(({ h }) => h.role === 'user' && h.content === prevUser.content)?.idx ?? -1
                const historyBeforeThisTurn = lastUserIdx >= 0 ? hist.slice(0, lastUserIdx) : []
                const { apiKey, baseUrl, model } = readSettings()
                const metadata = apiKey ? { api_key: apiKey, base_url: baseUrl, model } : undefined
                retryFn = () => send(prevUser.content, historyBeforeThisTurn, metadata)
              }
            }
            return <MessageBubble key={msg.id} message={msg} onRetry={retryFn} />
          })}
          <div ref={bottomRef} />
        </div>
      </div>

      {/* Input area */}
      <div className="border-t border-surface-800 px-4 py-4 bg-surface-900/50 backdrop-blur-sm">
        <div className="max-w-3xl mx-auto">
          <div className={`flex gap-3 items-end bg-surface-800 rounded-2xl border transition-colors px-4 py-3 ${isSending ? 'border-brand-500/30' : 'border-surface-700 focus-within:border-brand-500/50'
            }`}>
            <textarea
              ref={textareaRef}
              value={input}
              onChange={handleInput}
              onKeyDown={handleKeyDown}
              placeholder="Message the agent… (Enter to send, Shift+Enter for newline)"
              rows={1}
              disabled={isSending}
              className="flex-1 bg-transparent text-sm text-white placeholder-surface-600 outline-none resize-none leading-relaxed disabled:opacity-50"
              style={{ maxHeight: '200px' }}
            />

            {isSending ? (
              <button
                onClick={cancel}
                className="flex-shrink-0 p-1.5 rounded-lg text-surface-400 hover:text-red-400 hover:bg-red-400/10 transition-colors"
                title="Cancel"
              >
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            ) : (
              <button
                onClick={handleSend}
                disabled={!input.trim()}
                className="flex-shrink-0 p-1.5 rounded-lg bg-brand-600 hover:bg-brand-500 disabled:bg-surface-700 disabled:text-surface-500 text-white transition-colors"
                title="Send (Enter)"
              >
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M12 19l9 2-9-18-9 18 9-2zm0 0v-8" />
                </svg>
              </button>
            )}
          </div>
          <p className="text-xs text-surface-700 mt-2 text-center">
            Shift+Enter for new line · Enter to send
          </p>
        </div>
      </div>
    </div>
  )
}
