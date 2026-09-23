'use client'

import {
  CheckCircle2,
  Download,
  LoaderCircle,
  RefreshCw,
  RotateCw,
} from 'lucide-react'

import type { AppUpdateState } from '@/components/use-app-updater'

type UpdateSettingsProps = {
  state: AppUpdateState
  onCheck: () => Promise<void>
  onInstall: () => Promise<void>
}

const formatBytes = (bytes: number) => {
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export const UpdateSettings = ({
  onCheck,
  onInstall,
  state,
}: UpdateSettingsProps) => {
  const isWorking =
    state.status === 'checking' ||
    state.status === 'downloading' ||
    state.status === 'installing'
  const progress =
    state.totalBytes && state.totalBytes > 0
      ? Math.min(100, (state.downloadedBytes / state.totalBytes) * 100)
      : null

  return (
    <fieldset className="update-settings">
      <legend>
        <RefreshCw aria-hidden="true" size={17} /> Alkalmazásfrissítés
      </legend>

      <div className="update-summary">
        <div>
          <strong>
            {state.status === 'available'
              ? `Elérhető az ERGO ${state.availableVersion}`
              : 'Az ERGO naprakészen tartása'}
          </strong>
          <span>
            {state.currentVersion
              ? `Telepített verzió: ${state.currentVersion}`
              : 'A verzió ellenőrzése az asztali alkalmazásban érhető el.'}
          </span>
        </div>
        {state.status === 'current' ? (
          <CheckCircle2
            aria-label="Naprakész"
            className="update-ok"
            size={22}
          />
        ) : null}
      </div>

      {state.notes && state.status === 'available' ? (
        <p className="update-notes">{state.notes}</p>
      ) : null}

      {state.status === 'downloading' || state.status === 'installing' ? (
        <div className="update-progress" aria-live="polite">
          <div className="update-progress-track">
            <span style={{ width: `${progress ?? 12}%` }} />
          </div>
          <span>
            {state.status === 'installing'
              ? 'Telepítés és újraindítás…'
              : progress !== null
                ? `${Math.round(progress)}% · ${formatBytes(state.downloadedBytes)}`
                : `${formatBytes(state.downloadedBytes)} letöltve`}
          </span>
        </div>
      ) : null}

      {state.error ? (
        <p className="settings-error" role="alert">
          {state.error}
        </p>
      ) : null}

      <div className="update-actions">
        <button
          disabled={isWorking}
          onClick={() => void onCheck()}
          type="button"
        >
          {state.status === 'checking' ? (
            <LoaderCircle aria-hidden="true" className="spin" size={15} />
          ) : (
            <RefreshCw aria-hidden="true" size={15} />
          )}
          Frissítés keresése
        </button>
        {state.status === 'available' ? (
          <button
            className="update-install-button"
            onClick={() => void onInstall()}
            type="button"
          >
            <Download aria-hidden="true" size={16} />
            Frissítés és újraindítás
          </button>
        ) : null}
        {state.status === 'installing' ? (
          <span className="update-restart-label">
            <RotateCw aria-hidden="true" className="spin" size={15} />
            Újraindítás…
          </span>
        ) : null}
      </div>
    </fieldset>
  )
}
