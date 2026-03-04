import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import hljs from 'highlight.js'
import type { ChatMessage } from '../store'
import { EventTimeline } from './EventTimeline'

interface MessageBubbleProps {
  message: ChatMessage
  onRetry?: () => void
}

export function MessageBubble({ message, onRetry }: MessageBubbleProps) {
  if (message.role === 'user') {
    return <UserBubble message={message} />
  }
  return <AssistantBubble message={message} onRetry={onRetry} />
}

function UserBubble({ message }: MessageBubbleProps) {
  return (
    <div className="flex justify-end mb-6">
      <div className="max-w-[80%]">
        <div className="bg-brand-600 text-white rounded-2xl rounded-br-sm px-4 py-3 text-sm leading-relaxed shadow-sm">
          {message.content}
        </div>
      </div>
    </div>
  )
}

function AssistantBubble({ message, onRetry }: MessageBubbleProps) {
  const [copied, setCopied] = useState(false)

  function copy() {
    navigator.clipboard.writeText(message.content).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    })
  }

  return (
    <div className="flex gap-3 mb-6 group">
      {/* Avatar */}
      <div className="flex-shrink-0 w-7 h-7 rounded-full bg-brand-600/20 border border-brand-500/30 flex items-center justify-center text-brand-400 text-xs font-bold mt-0.5">
        R
      </div>

      <div className="flex-1 min-w-0">
        {/* Thinking block — live spinner while isThinking, collapsible summary after */}
        {(message.isThinking || message.thinkingContent) && (
          <ThinkingBlock
            isLive={message.isThinking ?? false}
            content={message.thinkingContent}
          />
        )}

        {/* Main content */}
        {(message.content || (message.streaming && !message.isThinking)) && (
          <div className="relative">
            <div
              className={`prose prose-invert prose-sm max-w-none text-surface-100 leading-relaxed ${message.streaming && !message.content ? 'cursor-blink' : ''
                }`}
            >
              {message.content ? (
                <MarkdownContent
                  content={message.content}
                  isStreaming={message.streaming ?? false}
                />
              ) : message.streaming ? (
                <span className="cursor-blink text-surface-400 text-sm">Thinking</span>
              ) : null}
            </div>

            {/* Copy button */}
            {!message.streaming && message.content && (
              <button
                onClick={copy}
                className="absolute top-0 right-0 opacity-0 group-hover:opacity-100 transition-opacity p-1 text-surface-500 hover:text-surface-300 rounded"
                title="Copy"
              >
                {copied ? (
                  <svg className="w-3.5 h-3.5 text-green-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                ) : (
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                  </svg>
                )}
              </button>
            )}
          </div>
        )}

        {/* Tool calls timeline — shown after text */}
        {message.toolCalls.length > 0 && (
          <div className="mt-3">
            <EventTimeline toolCalls={message.toolCalls} />
          </div>
        )}

        {/* Error state */}
        {message.error && (
          <div className="mt-2 flex items-center gap-2 text-red-400 text-sm bg-red-500/10 border border-red-500/20 rounded-lg px-3 py-2">
            <svg className="w-4 h-4 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
            </svg>
            <span className="flex-1">{message.error}</span>
            {onRetry && (
              <button
                onClick={onRetry}
                className="flex-shrink-0 flex items-center gap-1 text-xs text-surface-400 hover:text-white border border-surface-600 hover:border-surface-400 rounded px-2 py-0.5 transition-colors"
                title="Retry"
              >
                <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                  <path strokeLinecap="round" strokeLinejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                </svg>
                Retry
              </button>
            )}
          </div>
        )}

        {/* Usage */}
        {message.usage && !message.streaming && (
          <div className="mt-2 text-xs text-surface-600">
            {message.usage.prompt_tokens + message.usage.completion_tokens} tokens
          </div>
        )}
      </div>
    </div>
  )
}

// ── ThinkingBlock ─────────────────────────────────────────────────────────────

function ThinkingBlock({ isLive, content }: { isLive: boolean; content?: string }) {
  const [expanded, setExpanded] = useState(false)

  if (isLive) {
    return (
      <div className="flex items-center gap-2 mb-3 text-xs text-violet-400">
        <svg className="w-3.5 h-3.5 animate-spin flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
        </svg>
        <span className="italic">Thinking…</span>
      </div>
    )
  }

  if (!content) return null

  const wordCount = content.split(/\s+/).filter(Boolean).length

  return (
    <div className="mb-3 rounded-lg border border-violet-500/20 bg-violet-950/20 overflow-hidden text-xs">
      <button
        onClick={() => setExpanded(v => !v)}
        className="w-full flex items-center gap-2 px-3 py-2 text-violet-400 hover:text-violet-300 hover:bg-violet-500/10 transition-colors text-left"
      >
        <svg
          className={`w-3 h-3 flex-shrink-0 transition-transform ${expanded ? 'rotate-90' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
        <svg className="w-3.5 h-3.5 flex-shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
          <path strokeLinecap="round" strokeLinejoin="round" d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z" />
        </svg>
        <span className="italic">Thought for ~{wordCount} words</span>
      </button>
      {expanded && (
        <pre className="px-3 pb-3 text-violet-300/70 whitespace-pre-wrap font-mono leading-relaxed max-h-72 overflow-y-auto text-xs">
          {content}
        </pre>
      )}
    </div>
  )
}

// ── Markdown renderer with syntax highlighting ────────────────────────────────

function MarkdownContent({ content, isStreaming }: { content: string; isStreaming: boolean }) {
  const codeRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!isStreaming && codeRef.current) {
      codeRef.current.querySelectorAll('pre code').forEach(block => {
        hljs.highlightElement(block as HTMLElement)
      })
    }
  }, [content, isStreaming])

  return (
    <div ref={codeRef} className={isStreaming ? 'cursor-blink' : ''}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          code({ children, className }) {
            const isInline = !className
            if (isInline) {
              return (
                <code className="bg-surface-800 text-brand-300 px-1 py-0.5 rounded text-xs font-mono">
                  {children}
                </code>
              )
            }
            return (
              <code className={`${className ?? ''} font-mono`}>{children}</code>
            )
          },
          pre({ children }) {
            return (
              <pre className="bg-surface-900 border border-surface-700 rounded-lg overflow-x-auto p-4 my-3 text-xs font-mono">
                {children}
              </pre>
            )
          },
          a({ href, children }) {
            return (
              <a href={href} target="_blank" rel="noreferrer" className="text-brand-400 hover:text-brand-300 underline underline-offset-2">
                {children}
              </a>
            )
          },
          table({ children }) {
            return (
              <div className="overflow-x-auto my-3">
                <table className="w-full text-xs border-collapse">{children}</table>
              </div>
            )
          },
          th({ children }) {
            return <th className="border border-surface-700 bg-surface-800 px-3 py-1.5 text-left text-surface-200 font-medium">{children}</th>
          },
          td({ children }) {
            return <td className="border border-surface-700 px-3 py-1.5 text-surface-300">{children}</td>
          },
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  )
}
