import { useState, useEffect } from 'react'
import type { LLMSettings } from '../hooks/useSettings'

interface Props {
  open: boolean
  settings: LLMSettings
  onSave: (settings: LLMSettings) => void
  onClose: () => void
}

export function SettingsModal({ open, settings, onSave, onClose }: Props) {
  const [apiKey, setApiKey] = useState(settings.apiKey)
  const [baseUrl, setBaseUrl] = useState(settings.baseUrl)
  const [model, setModel] = useState(settings.model)
  const [showKey, setShowKey] = useState(false)

  // Sync when settings prop changes (e.g. loaded from localStorage)
  useEffect(() => {
    setApiKey(settings.apiKey)
    setBaseUrl(settings.baseUrl)
    setModel(settings.model)
  }, [settings])

  if (!open) return null

  function handleSave() {
    onSave({ apiKey: apiKey.trim(), baseUrl: baseUrl.trim(), model: model.trim() })
    onClose()
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Escape') onClose()
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) handleSave()
  }

  const presets = [
    { label: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o' },
    { label: 'Anthropic', baseUrl: 'https://api.anthropic.com/v1', model: 'claude-sonnet-4-5' },
    { label: 'Local (Ollama)', baseUrl: 'http://localhost:11434/v1', model: 'llama3.2' },
  ]

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onClick={(e) => { if (e.target === e.currentTarget) onClose() }}
      onKeyDown={handleKeyDown}
    >
      <div className="bg-surface-900 border border-surface-700 rounded-2xl shadow-2xl w-full max-w-md">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-surface-800">
          <div className="flex items-center gap-2">
            <svg className="w-4 h-4 text-brand-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M10.343 3.94c.09-.542.56-.94 1.11-.94h1.093c.55 0 1.02.398 1.11.94l.149.894c.07.424.384.764.78.93.398.164.855.142 1.205-.108l.737-.527a1.125 1.125 0 011.45.12l.773.774c.39.389.44 1.002.12 1.45l-.527.737c-.25.35-.272.806-.107 1.204.165.397.505.71.93.78l.893.15c.543.09.94.56.94 1.109v1.094c0 .55-.397 1.02-.94 1.11l-.893.149c-.425.07-.765.383-.93.78-.165.398-.143.854.107 1.204l.527.738c.32.447.269 1.06-.12 1.45l-.774.773a1.125 1.125 0 01-1.449.12l-.738-.527c-.35-.25-.806-.272-1.203-.107-.397.165-.71.505-.781.929l-.149.894c-.09.542-.56.94-1.11.94h-1.094c-.55 0-1.019-.398-1.11-.94l-.148-.894c-.071-.424-.384-.764-.781-.93-.398-.164-.854-.142-1.204.108l-.738.527c-.447.32-1.06.269-1.45-.12l-.773-.774a1.125 1.125 0 01-.12-1.45l.527-.737c.25-.35.273-.806.108-1.204-.165-.397-.505-.71-.93-.78l-.894-.15c-.542-.09-.94-.56-.94-1.109v-1.094c0-.55.398-1.02.94-1.11l.894-.149c.424-.07.765-.383.93-.78.165-.398.143-.854-.108-1.204l-.526-.738a1.125 1.125 0 01.12-1.45l.773-.773a1.125 1.125 0 011.45-.12l.737.527c.35.25.807.272 1.204.107.397-.165.71-.505.78-.929l.15-.894z" />
              <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
            <h2 className="text-sm font-semibold text-white">LLM Settings</h2>
          </div>
          <button
            className="p-1.5 text-surface-500 hover:text-white rounded-lg hover:bg-surface-800 transition-colors"
            onClick={onClose}
          >
            <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Presets */}
        <div className="px-6 pt-4">
          <p className="text-xs text-surface-500 mb-2">Presets</p>
          <div className="flex gap-2">
            {presets.map(p => (
              <button
                key={p.label}
                className="px-3 py-1.5 text-xs rounded-lg bg-surface-800 hover:bg-surface-700 text-surface-300 hover:text-white border border-surface-700 hover:border-surface-600 transition-colors"
                onClick={() => { setBaseUrl(p.baseUrl); setModel(p.model) }}
              >
                {p.label}
              </button>
            ))}
          </div>
        </div>

        {/* Form */}
        <div className="px-6 py-4 flex flex-col gap-4">
          {/* API Key */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-surface-400">API Key</label>
            <div className="relative">
              <input
                type={showKey ? 'text' : 'password'}
                value={apiKey}
                onChange={e => setApiKey(e.target.value)}
                placeholder="sk-..."
                className="w-full bg-surface-800 border border-surface-700 rounded-xl px-3 py-2.5 pr-9 text-sm text-white placeholder-surface-600 focus:outline-none focus:border-brand-500/60 focus:ring-1 focus:ring-brand-500/30 transition-colors"
              />
              <button
                type="button"
                className="absolute right-2.5 top-1/2 -translate-y-1/2 text-surface-500 hover:text-surface-300"
                onClick={() => setShowKey(v => !v)}
                tabIndex={-1}
              >
                {showKey ? (
                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M3.98 8.223A10.477 10.477 0 001.934 12C3.226 16.338 7.244 19.5 12 19.5c.993 0 1.953-.138 2.863-.395M6.228 6.228A10.45 10.45 0 0112 4.5c4.756 0 8.773 3.162 10.065 7.498a10.523 10.523 0 01-4.293 5.774M6.228 6.228L3 3m3.228 3.228l3.65 3.65m7.894 7.894L21 21m-3.228-3.228l-3.65-3.65m0 0a3 3 0 10-4.243-4.243m4.242 4.242L9.88 9.88" />
                  </svg>
                ) : (
                  <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M2.036 12.322a1.012 1.012 0 010-.639C3.423 7.51 7.36 4.5 12 4.5c4.638 0 8.573 3.007 9.963 7.178.07.207.07.431 0 .639C20.577 16.49 16.64 19.5 12 19.5c-4.638 0-8.573-3.007-9.963-7.178z" />
                    <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                  </svg>
                )}
              </button>
            </div>
          </div>

          {/* Base URL */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-surface-400">Base URL</label>
            <input
              type="text"
              value={baseUrl}
              onChange={e => setBaseUrl(e.target.value)}
              placeholder="https://api.openai.com/v1"
              className="w-full bg-surface-800 border border-surface-700 rounded-xl px-3 py-2.5 text-sm text-white placeholder-surface-600 focus:outline-none focus:border-brand-500/60 focus:ring-1 focus:ring-brand-500/30 transition-colors"
            />
          </div>

          {/* Model */}
          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-medium text-surface-400">Model</label>
            <input
              type="text"
              value={model}
              onChange={e => setModel(e.target.value)}
              placeholder="gpt-4o"
              className="w-full bg-surface-800 border border-surface-700 rounded-xl px-3 py-2.5 text-sm text-white placeholder-surface-600 focus:outline-none focus:border-brand-500/60 focus:ring-1 focus:ring-brand-500/30 transition-colors"
            />
          </div>
        </div>

        {/* Footer */}
        <div className="px-6 pb-5 flex justify-end gap-2">
          <button
            className="px-4 py-2 text-sm rounded-xl bg-surface-800 hover:bg-surface-700 text-surface-300 hover:text-white border border-surface-700 transition-colors"
            onClick={onClose}
          >
            Cancel
          </button>
          <button
            className="px-4 py-2 text-sm rounded-xl bg-brand-600 hover:bg-brand-500 text-white font-medium transition-colors"
            onClick={handleSave}
          >
            Save
          </button>
        </div>
      </div>
    </div>
  )
}
