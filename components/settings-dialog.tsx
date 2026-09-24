'use client'

import { KeyRound, Settings2, X } from 'lucide-react'
import { FormEvent, useState } from 'react'

import { checkDatabase, saveSettings } from '@/lib/desktop'
import type { DesktopSettings, SettingsInput } from '@/lib/types'
import type { AppUpdateState } from '@/components/use-app-updater'
import { UpdateSettings } from '@/components/update-settings'

type SettingsDialogProps = {
  settings: DesktopSettings | null
  updateState: AppUpdateState
  onCheckForUpdate: () => Promise<void>
  onInstallUpdate: () => Promise<void>
  onClose: () => void
  onSaved: () => Promise<void>
}

const defaults: SettingsInput = {
  mysqlHost: '10.21.36.17',
  mysqlPort: 3306,
  mysqlDatabase: 'treu_replica',
  mysqlUser: 'TREU',
  mysqlSsl: false,
  mysqlPassword: '',
  aiBaseUrl: 'https://api.openai.com/v1',
  aiModel: 'gpt-6-sol',
  analysisFyWindow: 5,
  aiApiKey: '',
}

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'A beállítások mentése nem sikerült.'
}

export const SettingsDialog = ({
  onCheckForUpdate,
  onClose,
  onInstallUpdate,
  onSaved,
  settings,
  updateState,
}: SettingsDialogProps) => {
  const [form, setForm] = useState<SettingsInput>(() =>
    settings
      ? {
          mysqlHost: settings.mysqlHost,
          mysqlPort: settings.mysqlPort,
          mysqlDatabase: settings.mysqlDatabase,
          mysqlUser: settings.mysqlUser,
          mysqlSsl: settings.mysqlSsl,
          mysqlPassword: '',
          aiBaseUrl: settings.aiBaseUrl,
          aiModel: settings.aiModel,
          analysisFyWindow: settings.analysisFyWindow,
          aiApiKey: '',
        }
      : defaults,
  )
  const [error, setError] = useState<string | null>(null)
  const [isSaving, setIsSaving] = useState(false)

  const update = <K extends keyof SettingsInput>(
    key: K,
    value: SettingsInput[K],
  ) => setForm((current) => ({ ...current, [key]: value }))

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    setError(null)
    setIsSaving(true)

    try {
      await saveSettings(form)
      await checkDatabase()
      await onSaved()
      onClose()
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setIsSaving(false)
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        aria-labelledby="settings-title"
        aria-modal="true"
        className="settings-dialog"
        role="dialog"
      >
        <header>
          <div>
            <p className="eyebrow accent">HELYI KAPCSOLAT</p>
            <h2 id="settings-title">ERGO beállítások</h2>
          </div>
          <button aria-label="Bezárás" onClick={onClose} type="button">
            <X aria-hidden="true" size={19} />
          </button>
        </header>

        <form onSubmit={handleSubmit}>
          <fieldset>
            <legend>
              <Settings2 aria-hidden="true" size={17} /> Adatbázis
            </legend>
            <div className="settings-grid database-settings">
              <label>
                Szerver
                <input
                  required
                  value={form.mysqlHost}
                  onChange={(event) => update('mysqlHost', event.target.value)}
                />
              </label>
              <label>
                Port
                <input
                  max={65535}
                  min={1}
                  required
                  type="number"
                  value={form.mysqlPort}
                  onChange={(event) =>
                    update('mysqlPort', Number(event.target.value))
                  }
                />
              </label>
              <label>
                Adatbázis
                <input
                  required
                  value={form.mysqlDatabase}
                  onChange={(event) =>
                    update('mysqlDatabase', event.target.value)
                  }
                />
              </label>
              <label>
                Felhasználó
                <input
                  required
                  value={form.mysqlUser}
                  onChange={(event) => update('mysqlUser', event.target.value)}
                />
              </label>
              <label className="settings-wide">
                MySQL-jelszó
                <input
                  autoComplete="new-password"
                  placeholder={
                    settings?.hasMysqlPassword
                      ? 'Mentve a Kulcskarikában — hagyd üresen a megtartáshoz'
                      : 'Kötelező az első beállításkor'
                  }
                  type="password"
                  value={form.mysqlPassword}
                  onChange={(event) =>
                    update('mysqlPassword', event.target.value)
                  }
                />
              </label>
              <label className="checkbox-label settings-wide">
                <input
                  checked={form.mysqlSsl}
                  type="checkbox"
                  onChange={(event) => update('mysqlSsl', event.target.checked)}
                />
                A MySQL-kapcsolat TLS-t használ
              </label>
            </div>
          </fieldset>

          <fieldset>
            <legend>
              <KeyRound aria-hidden="true" size={17} /> AI-szolgáltatás
            </legend>
            <div className="settings-grid">
              <label>
                OpenAI-kompatibilis API-cím
                <input
                  required
                  type="url"
                  value={form.aiBaseUrl}
                  onChange={(event) => update('aiBaseUrl', event.target.value)}
                />
              </label>
              <label>
                Modell
                <input
                  required
                  value={form.aiModel}
                  onChange={(event) => update('aiModel', event.target.value)}
                />
              </label>
              <label>
                Alapértelmezett időablak
                <select
                  value={form.analysisFyWindow}
                  onChange={(event) =>
                    update(
                      'analysisFyWindow',
                      Number(event.target.value) as 1 | 3 | 5,
                    )
                  }
                >
                  <option value={1}>1 FY</option>
                  <option value={3}>3 FY</option>
                  <option value={5}>5 FY</option>
                </select>
              </label>
              <label>
                API-kulcs
                <input
                  autoComplete="new-password"
                  placeholder={
                    settings?.hasAiApiKey
                      ? 'Mentve a Kulcskarikában — hagyd üresen a megtartáshoz'
                      : 'Kötelező az első beállításkor'
                  }
                  type="password"
                  value={form.aiApiKey}
                  onChange={(event) => update('aiApiKey', event.target.value)}
                />
              </label>
            </div>
          </fieldset>

          <UpdateSettings
            onCheck={onCheckForUpdate}
            onInstall={onInstallUpdate}
            state={updateState}
          />

          <p className="settings-note">
            Az itt megadott titkok a macOS Kulcskarikába kerülnek. A gépenkénti
            alkalmazáskonfigurációs mappában lévő `.env` fájl értékei
            elsőbbséget élveznek. Az elemzéshez szükséges séma és lekérdezési
            eredmény az itt megadott AI-szolgáltatóhoz kerül feldolgozásra.
          </p>
          {error ? <p className="settings-error">{error}</p> : null}
          <div className="settings-actions">
            <button onClick={onClose} type="button">
              Mégse
            </button>
            <button disabled={isSaving} type="submit">
              {isSaving ? 'Kapcsolat ellenőrzése…' : 'Mentés és ellenőrzés'}
            </button>
          </div>
        </form>
      </section>
    </div>
  )
}
