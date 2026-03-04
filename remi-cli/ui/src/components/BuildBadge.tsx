import type { BuildStatus } from '../hooks/useStatus'

interface BuildBadgeProps {
  status: BuildStatus
}

export function BuildBadge({ status }: BuildBadgeProps) {
  if (status.kind === 'idle') return null

  const configs = {
    building: {
      bg: 'bg-yellow-500/10 border-yellow-500/30',
      text: 'text-yellow-300',
      dot: 'bg-yellow-400 animate-pulse',
      label: 'Building…',
    },
    ok: {
      bg: 'bg-green-500/10 border-green-500/30',
      text: 'text-green-300',
      dot: 'bg-green-400',
      label: `Built`,
    },
    error: {
      bg: 'bg-red-500/10 border-red-500/30',
      text: 'text-red-300',
      dot: 'bg-red-400',
      label: 'Build failed',
    },
    reloaded: {
      bg: 'bg-brand-500/10 border-brand-500/30',
      text: 'text-brand-300',
      dot: 'bg-brand-400 animate-ping',
      label: 'Agent reloaded',
    },
  }

  const cfg = configs[status.kind]

  return (
    <div
      className={`flex items-center gap-1.5 px-2.5 py-1 rounded-full border text-xs font-medium ${cfg.bg} ${cfg.text}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${cfg.dot}`} />
      {cfg.label}
      {status.kind === 'error' && (
        <span
          title={status.message}
          className="ml-1 cursor-help underline underline-offset-2 decoration-dotted"
        >
          ?
        </span>
      )}
    </div>
  )
}
