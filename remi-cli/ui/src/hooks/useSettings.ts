import { useState, useCallback } from 'react'

const SETTINGS_KEY = 'remi_settings'

export interface LLMSettings {
  apiKey: string
  baseUrl: string
  model: string
}

const DEFAULT_SETTINGS: LLMSettings = {
  apiKey: '',
  baseUrl: 'https://api.openai.com/v1',
  model: 'gpt-4o',
}

function load(): LLMSettings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY)
    if (!raw) return DEFAULT_SETTINGS
    return { ...DEFAULT_SETTINGS, ...JSON.parse(raw) }
  } catch {
    return DEFAULT_SETTINGS
  }
}

export function useSettings() {
  const [settings, setSettingsState] = useState<LLMSettings>(load)

  const saveSettings = useCallback((next: LLMSettings) => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(next))
    setSettingsState(next)
  }, [])

  return { settings, saveSettings }
}

export function readSettings(): LLMSettings {
  return load()
}
