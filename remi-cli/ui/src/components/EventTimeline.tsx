import { useState } from 'react'
import type { ToolCallState } from '../store'

interface EventTimelineProps {
  toolCalls: ToolCallState[]
}

export function EventTimeline({ toolCalls }: EventTimelineProps) {
  const [expanded, setExpanded] = useState(false)

  if (toolCalls.length === 0) return null

  return (
    <div className="my-2">
      <button
        onClick={() => setExpanded(v => !v)}
        className="flex items-center gap-1.5 text-xs text-surface-400 hover:text-surface-200 transition-colors"
      >
        <svg
          className={`w-3.5 h-3.5 transition-transform ${expanded ? 'rotate-90' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
        {toolCalls.length} tool call{toolCalls.length > 1 ? 's' : ''}
        {!expanded && (
          <span className="ml-1 text-surface-500">
            ({toolCalls.map(tc => tc.name).join(', ')})
          </span>
        )}
      </button>

      {expanded && (
        <div className="mt-2 space-y-2 border-l-2 border-surface-700 pl-3">
          {toolCalls.map(tc => (
            <ToolCallItem key={tc.id} toolCall={tc} />
          ))}
        </div>
      )}
    </div>
  )
}

function ToolCallItem({ toolCall }: { toolCall: ToolCallState }) {
  const [showArgs, setShowArgs] = useState(false)

  // Parse args for pretty display
  let argsDisplay: string
  try {
    argsDisplay = JSON.stringify(JSON.parse(toolCall.argumentsDelta), null, 2)
  } catch {
    argsDisplay = toolCall.argumentsDelta
  }

  let resultDisplay: string | undefined
  if (toolCall.result !== undefined) {
    try {
      resultDisplay = JSON.stringify(JSON.parse(toolCall.result), null, 2)
    } catch {
      resultDisplay = toolCall.result
    }
  }

  return (
    <div className="text-xs space-y-1">
      {/* Tool name + status */}
      <div className="flex items-center gap-2">
        <span className="font-mono font-medium text-brand-300">{toolCall.name}</span>
        {toolCall.result !== undefined ? (
          <span className="text-green-400 flex items-center gap-0.5">
            <svg className="w-3 h-3" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
            </svg>
            done
          </span>
        ) : (
          <span className="text-yellow-400 flex items-center gap-0.5">
            <svg className="w-3 h-3 animate-spin" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
            </svg>
            running
          </span>
        )}
        <button
          onClick={() => setShowArgs(v => !v)}
          className="text-surface-500 hover:text-surface-300 ml-auto"
        >
          {showArgs ? 'hide' : 'args'}
        </button>
      </div>

      {/* Arguments */}
      {showArgs && argsDisplay && (
        <pre className="text-xs font-mono text-surface-300 bg-surface-900 rounded p-2 overflow-x-auto leading-relaxed">
          {argsDisplay}
        </pre>
      )}

      {/* Result */}
      {toolCall.result !== undefined && (
        <div>
          <details className="group">
            <summary className="text-surface-500 hover:text-surface-300 cursor-pointer list-none flex items-center gap-1">
              <svg
                className="w-3 h-3 transition-transform group-open:rotate-90"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
              </svg>
              result
            </summary>
            <pre className="mt-1 text-xs font-mono text-surface-300 bg-surface-900 rounded p-2 overflow-x-auto leading-relaxed max-h-48 overflow-y-auto">
              {resultDisplay}
            </pre>
          </details>
        </div>
      )}
    </div>
  )
}
