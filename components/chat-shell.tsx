'use client'

import {
  ArrowUp,
  BarChart3,
  Database,
  FileSpreadsheet,
  LoaderCircle,
  MessageSquarePlus,
  Settings,
  ShieldCheck,
  Sparkles,
  TrendingUp,
} from 'lucide-react'

import { MessageView } from '@/components/message-view'
import { SettingsDialog } from '@/components/settings-dialog'
import { useAppUpdater } from '@/components/use-app-updater'
import { useErpChat } from '@/components/use-erp-chat'

const quickPrompts = [
  {
    icon: TrendingUp,
    label: 'Árbevétel trendje',
    prompt: 'Mutasd meg az elmúlt 12 hónap árbevételének havi trendjét.',
  },
  {
    icon: BarChart3,
    label: 'Top ügyfelek',
    prompt: 'Melyik 10 ügyfél hozta a legtöbb árbevételt idén?',
  },
  {
    icon: FileSpreadsheet,
    label: 'Kintlévőségek',
    prompt: 'Készíts összefoglalót a jelenlegi lejárt kintlévőségekről.',
  },
]

const statusLabels = {
  checking: 'Kapcsolódás…',
  online: 'Élő adatkapcsolat',
  offline: 'Nincs adatkapcsolat',
}

const modelLabels: Record<string, string> = {
  'gpt-6-astra': 'Astra — legerősebb',
  'gpt-5.6-sol': 'Sol — megbízható',
  'gpt-5.6-terra': 'Terra — kiegyensúlyozott',
  'gpt-5.6-luna': 'Luna — gyors',
  'gpt-5.5': 'GPT-5.5',
}

const modelLabel = (model: string) => modelLabels[model] ?? model

export const ChatShell = () => {
  const updater = useAppUpdater()
  const {
    availableModels,
    changeModel,
    clearConversation,
    databaseStatus,
    error,
    handleKeyDown,
    handleSubmit,
    input,
    messages,
    modelsLoading,
    refreshSettings,
    setInput,
    setSettingsOpen,
    settings,
    settingsOpen,
    status,
    submitMessage,
  } = useErpChat()
  const isBusy = status === 'submitted'

  return (
    <>
      <div className="app-shell">
        <aside className="sidebar">
          <div className="sidebar-brand">
            <div className="brand-mark small">E</div>
            <div>
              <p className="eyebrow">TRANS-EUROPE</p>
              <p className="brand-name">Ergo</p>
            </div>
          </div>

          <button
            className="new-chat-button"
            onClick={clearConversation}
            type="button"
          >
            <MessageSquarePlus aria-hidden="true" size={17} />
            Új elemzés
          </button>

          <nav className="prompt-nav" aria-label="Gyors kérdések">
            <p className="nav-label">GYORS KÉRDÉSEK</p>
            {quickPrompts.map(({ icon: Icon, label, prompt }) => (
              <button
                disabled={isBusy}
                key={label}
                onClick={() => void submitMessage(prompt)}
                type="button"
              >
                <Icon aria-hidden="true" size={16} />
                <span>{label}</span>
              </button>
            ))}
          </nav>

          <div className="sidebar-spacer" />

          <div className="safety-note">
            <ShieldCheck aria-hidden="true" size={18} />
            <div>
              <strong>Csak olvasható</strong>
              <span>Az Ergo nem módosíthat vállalati adatot.</span>
            </div>
          </div>

          <button
            className={`logout-button ${
              updater.state.status === 'available' ? 'has-update' : ''
            }`}
            onClick={() => setSettingsOpen(true)}
            type="button"
          >
            <Settings aria-hidden="true" size={16} />
            Kapcsolati beállítások
            {updater.state.status === 'available' ? (
              <span className="update-dot" aria-label="Frissítés érhető el" />
            ) : null}
          </button>
        </aside>

        <main className="chat-main">
          <header className="chat-header">
            <div>
              <p className="header-kicker">ÉLŐ ERP ELEMZŐ</p>
              <h1>Üzleti áttekintés</h1>
            </div>
            <div className="header-actions">
              <label className="model-picker">
                <Sparkles aria-hidden="true" size={15} />
                <span>Modell</span>
                <select
                  aria-label="AI-modell kiválasztása"
                  disabled={isBusy || modelsLoading || !settings}
                  value={settings?.aiModel ?? ''}
                  onChange={(event) => void changeModel(event.target.value)}
                >
                  {availableModels.map((model) => (
                    <option key={model} value={model}>
                      {modelLabel(model)}
                    </option>
                  ))}
                </select>
              </label>
              <div className={`database-badge ${databaseStatus}`}>
                <span className="status-dot" />
                <Database aria-hidden="true" size={15} />
                {statusLabels[databaseStatus]}
              </div>
            </div>
          </header>

          <section className="conversation" aria-live="polite">
            {messages.length === 0 ? (
              <div className="empty-state">
                <div className="hero-icon">
                  <Sparkles aria-hidden="true" size={28} />
                </div>
                <p className="eyebrow accent">KÉRDEZZ AZ ADATOKTÓL</p>
                <h2>Miben segíthetek ma?</h2>
                <p className="empty-intro">
                  Írd le egyszerűen, mit szeretnél tudni. Az Ergo feltérképezi
                  az adatbázist, elkészíti a lekérdezést, majd érthetően
                  összefoglalja.
                </p>
                <div className="suggestion-grid">
                  {quickPrompts.map(({ icon: Icon, label, prompt }) => (
                    <button
                      disabled={isBusy}
                      key={label}
                      onClick={() => void submitMessage(prompt)}
                      type="button"
                    >
                      <Icon aria-hidden="true" size={19} />
                      <span>{label}</span>
                      <small>{prompt}</small>
                    </button>
                  ))}
                </div>
              </div>
            ) : (
              <div className="message-list">
                {messages.map((message) => (
                  <MessageView key={message.id} message={message} />
                ))}
                {status === 'submitted' ? (
                  <div className="assistant-thinking">
                    <div className="assistant-avatar">E</div>
                    <div className="thinking-bubble">
                      <span />
                      <span />
                      <span />
                      Elemzem az adatokat…
                    </div>
                  </div>
                ) : null}
              </div>
            )}
          </section>

          <div className="composer-wrap">
            {error ? <p className="chat-error">{error}</p> : null}
            <form className="composer" onSubmit={handleSubmit}>
              <textarea
                aria-label="Kérdés az ERP-adatokról"
                disabled={isBusy}
                placeholder="Kérdezz az ERP-adatokról…"
                rows={1}
                value={input}
                onChange={(event) => setInput(event.target.value)}
                onKeyDown={handleKeyDown}
              />
              {isBusy ? (
                <button aria-label="Elemzés folyamatban" disabled type="button">
                  <LoaderCircle aria-hidden="true" className="spin" size={17} />
                </button>
              ) : (
                <button
                  aria-label="Kérdés elküldése"
                  disabled={!input.trim()}
                  type="submit"
                >
                  <ArrowUp aria-hidden="true" size={19} />
                </button>
              )}
            </form>
            <p className="composer-hint">
              Enter: küldés · Shift + Enter: új sor · Az eredményeket mindig
              ellenőrizd üzleti döntés előtt.
            </p>
          </div>
        </main>
      </div>
      {settingsOpen ? (
        <SettingsDialog
          onCheckForUpdate={() => updater.checkForUpdate()}
          onClose={() => setSettingsOpen(false)}
          onInstallUpdate={updater.installUpdate}
          onSaved={refreshSettings}
          settings={settings}
          updateState={updater.state}
        />
      ) : null}
    </>
  )
}
