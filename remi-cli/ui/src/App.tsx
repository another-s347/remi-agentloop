import { useState } from 'react'
import { SessionProvider } from './store'
import { Sidebar } from './components/Sidebar'
import { ChatPane } from './components/ChatPane'
import { BuildBadge } from './components/BuildBadge'
import { SettingsModal } from './components/SettingsModal'
import { useStatus } from './hooks/useStatus'
import { useSettings } from './hooks/useSettings'

function AppInner() {
  const status = useStatus()
  const [sidebarOpen, setSidebarOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const { settings, saveSettings } = useSettings()

  return (
    <div className="flex h-full bg-surface-900 text-white font-sans overflow-hidden">
      {/* ── Desktop sidebar ────────────────────────────────────────── */}
      <div className="hidden md:flex">
        <Sidebar />
      </div>

      {/* ── Mobile sidebar overlay ─────────────────────────────────── */}
      {sidebarOpen && (
        <div className="md:hidden fixed inset-0 z-50 flex">
          <div
            className="absolute inset-0 bg-black/60"
            onClick={() => setSidebarOpen(false)}
          />
          <div className="relative z-10 flex">
            <Sidebar onClose={() => setSidebarOpen(false)} />
          </div>
        </div>
      )}

      {/* ── Main content ───────────────────────────────────────────── */}
      <div className="flex-1 flex flex-col min-w-0 min-h-0">
        {/* Header */}
        <header className="flex items-center justify-between px-4 py-2.5 border-b border-surface-800 bg-surface-950/50 backdrop-blur-sm z-10">
          {/* Mobile menu button */}
          <button
            className="md:hidden p-1.5 text-surface-400 hover:text-white rounded-lg hover:bg-surface-800"
            onClick={() => setSidebarOpen(true)}
          >
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
              <path strokeLinecap="round" strokeLinejoin="round" d="M4 6h16M4 12h16M4 18h16" />
            </svg>
          </button>

          <div className="flex items-center gap-2 md:hidden">
            <span className="text-brand-400 text-sm font-bold">remi</span>
            <span className="text-surface-400 text-xs">dev</span>
          </div>

          {/* Spacer */}
          <div className="hidden md:block" />

          <div className="flex items-center gap-2">
            {/* Settings button — shows orange dot when API key is not configured */}
            <button
              className={`relative p-1.5 rounded-lg hover:bg-surface-800 transition-colors ${settings.apiKey ? 'text-surface-500 hover:text-surface-300' : 'text-amber-400 hover:text-amber-300'}`}
              onClick={() => setSettingsOpen(true)}
              title={settings.apiKey ? 'LLM Settings' : 'Configure LLM API key'}
            >
              {!settings.apiKey && (
                <span className="absolute top-0.5 right-0.5 w-2 h-2 bg-amber-400 rounded-full animate-pulse" />
              )}
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M10.343 3.94c.09-.542.56-.94 1.11-.94h1.093c.55 0 1.02.398 1.11.94l.149.894c.07.424.384.764.78.93.398.164.855.142 1.205-.108l.737-.527a1.125 1.125 0 011.45.12l.773.774c.39.389.44 1.002.12 1.45l-.527.737c-.25.35-.272.806-.107 1.204.165.397.505.71.93.78l.893.15c.543.09.94.56.94 1.109v1.094c0 .55-.397 1.02-.94 1.11l-.893.149c-.425.07-.765.383-.93.78-.165.398-.143.854.107 1.204l.527.738c.32.447.269 1.06-.12 1.45l-.774.773a1.125 1.125 0 01-1.449.12l-.738-.527c-.35-.25-.806-.272-1.203-.107-.397.165-.71.505-.781.929l-.149.894c-.09.542-.56.94-1.11.94h-1.094c-.55 0-1.019-.398-1.11-.94l-.148-.894c-.071-.424-.384-.764-.781-.93-.398-.164-.854-.142-1.204.108l-.738.527c-.447.32-1.06.269-1.45-.12l-.773-.774a1.125 1.125 0 01-.12-1.45l.527-.737c-.25-.35.273-.806.108-1.204-.165-.397-.505-.71-.93-.78l-.894-.15c-.542-.09-.94-.56-.94-1.109v-1.094c0-.55.398-1.02.94-1.11l.894-.149c.424-.07.765-.383.93-.78.165-.398.143-.854-.108-1.204l-.526-.738a1.125 1.125 0 01.12-1.45l.773-.773a1.125 1.125 0 011.45-.12l.737.527c.35.25.807.272 1.204.107.397-.165.71-.505.78-.929l.15-.894z" />
                <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
              </svg>
            </button>
            {/* Build badge */}
            <BuildBadge status={status} />
          </div>
        </header>

        <SettingsModal
          open={settingsOpen}
          settings={settings}
          onSave={saveSettings}
          onClose={() => setSettingsOpen(false)}
        />

        {/* Chat pane */}
        <ChatPane />
      </div>
    </div>
  )
}

export default function App() {
  return (
    <SessionProvider>
      <AppInner />
    </SessionProvider>
  )
}
