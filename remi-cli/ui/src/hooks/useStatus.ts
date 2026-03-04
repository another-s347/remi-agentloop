import { useEffect, useRef, useState } from 'react'
import type { BuildStatusEvent } from '../types'

export type BuildStatus =
  | { kind: 'idle' }
  | { kind: 'building' }
  | { kind: 'ok'; crate_name: string }
  | { kind: 'error'; message: string }
  | { kind: 'reloaded'; crate_name: string }

export function useStatus() {
  const [status, setStatus] = useState<BuildStatus>({ kind: 'idle' })
  const esRef = useRef<EventSource | null>(null)

  useEffect(() => {
    const connect = () => {
      const es = new EventSource('/status')
      esRef.current = es

      const handle = (eventType: string, data: string) => {
        try {
          const parsed = JSON.parse(data) as BuildStatusEvent
          switch (parsed.type) {
            case 'build_start':
              setStatus({ kind: 'building' })
              break
            case 'build_ok':
              setStatus({ kind: 'ok', crate_name: parsed.crate_name })
              break
            case 'build_error':
              setStatus({ kind: 'error', message: parsed.message })
              break
            case 'agent_reloaded':
              setStatus({ kind: 'reloaded', crate_name: parsed.crate_name })
              // Reset to idle after 3s
              setTimeout(() => setStatus({ kind: 'idle' }), 3000)
              break
            default:
              break
          }
        } catch { }
      }

      // Listen to named events
      const events: (BuildStatusEvent['type'])[] = [
        'build_start',
        'build_ok',
        'build_error',
        'agent_reloaded',
        'ping',
      ]
      for (const eventType of events) {
        es.addEventListener(eventType, (e: MessageEvent) => handle(eventType, e.data))
      }

      es.onerror = () => {
        es.close()
        // Reconnect after 3s
        setTimeout(connect, 3000)
      }
    }

    connect()

    return () => {
      esRef.current?.close()
    }
  }, [])

  return status
}
