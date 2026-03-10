import { useState } from 'react'
import { useStore } from '../store'

interface SidebarProps {
  onClose?: () => void
}

export function Sidebar({ onClose }: SidebarProps) {
  const { state, dispatch, activeSession } = useStore()
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editingName, setEditingName] = useState('')

  function handleCreate() {
    dispatch({ type: 'CREATE_SESSION' })
  }

  function handleSelect(id: string) {
    dispatch({ type: 'SELECT_SESSION', sessionId: id })
    onClose?.()
  }

  function handleDelete(e: React.MouseEvent, id: string) {
    e.stopPropagation()
    if (confirm('Delete this session?')) {
      dispatch({ type: 'DELETE_SESSION', sessionId: id })
    }
  }

  function startRename(e: React.MouseEvent, id: string, name: string) {
    e.stopPropagation()
    setEditingId(id)
    setEditingName(name)
  }

  function commitRename(id: string) {
    if (editingName.trim()) {
      dispatch({ type: 'RENAME_SESSION', sessionId: id, name: editingName.trim() })
    }
    setEditingId(null)
  }

  return (
    <aside className="flex flex-col w-60 min-w-[240px] h-full bg-surface-950 border-r border-surface-800">
      {/* Logo / header */}
      <div className="flex items-center gap-2 px-4 py-4 border-b border-surface-800">
        <span className="text-brand-400 text-lg font-bold tracking-tight">remi</span>
        <span className="text-surface-200 text-sm font-medium opacity-60">dev</span>
      </div>

      {/* New chat button */}
      <div className="px-3 pt-3">
        <button
          onClick={handleCreate}
          className="w-full flex items-center gap-2 px-3 py-2 rounded-lg bg-brand-600 hover:bg-brand-500 text-white text-sm font-medium transition-colors"
        >
          <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4v16m8-8H4" />
          </svg>
          New chat
        </button>
      </div>

      {/* Session list */}
      <div className="flex-1 overflow-y-auto px-2 py-3 space-y-0.5">
        {state.sessions.length === 0 && (
          <p className="text-surface-600 text-xs px-2 py-4 text-center">No sessions yet</p>
        )}
        {state.sessions.map(session => (
          <div
            key={session.id}
            onClick={() => handleSelect(session.id)}
            className={`group relative flex items-center px-3 py-2 rounded-lg cursor-pointer text-sm transition-colors ${activeSession?.id === session.id
                ? 'bg-surface-800 text-white'
                : 'text-surface-200 hover:bg-surface-800/60'
              }`}
          >
            {editingId === session.id ? (
              <input
                autoFocus
                value={editingName}
                onClick={e => e.stopPropagation()}
                onChange={e => setEditingName(e.target.value)}
                onBlur={() => commitRename(session.id)}
                onKeyDown={e => {
                  if (e.key === 'Enter') commitRename(session.id)
                  if (e.key === 'Escape') setEditingId(null)
                }}
                className="flex-1 bg-transparent outline-none border-b border-brand-400 text-white"
              />
            ) : (
              <span className="flex-1 truncate">{session.name}</span>
            )}

            {/* Action icons — visible on hover */}
            {editingId !== session.id && (
              <span className="hidden group-hover:flex items-center gap-1 ml-1">
                <button
                  title="Rename"
                  onClick={e => startRename(e, session.id, session.name)}
                  className="text-surface-400 hover:text-white p-0.5 rounded"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M15.232 5.232l3.536 3.536M9 13l6.768-6.768a2 2 0 012.828 0l.172.172a2 2 0 010 2.828L12 15H9v-3z" />
                  </svg>
                </button>
                <button
                  title="Delete"
                  onClick={e => handleDelete(e, session.id)}
                  className="text-surface-400 hover:text-red-400 p-0.5 rounded"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6M9 7h6m2 0a1 1 0 01-1 1H8a1 1 0 01-1-1V4a1 1 0 011-1h8a1 1 0 011 1v3z" />
                  </svg>
                </button>
              </span>
            )}
          </div>
        ))}
      </div>

      {/* Footer */}
      <div className="px-4 py-3 border-t border-surface-800 text-xs text-surface-600">
        Sessions stored locally
      </div>
    </aside>
  )
}
