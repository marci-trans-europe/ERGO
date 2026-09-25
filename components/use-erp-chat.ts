'use client'

import {
  FormEvent,
  KeyboardEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from 'react'

import {
  analyzeErp,
  analyzeQuickErp,
  checkDatabase,
  listAiModels,
  loadSettings,
  selectAnalysisFyWindow,
  selectAiModel,
} from '@/lib/desktop'
import type {
  AnalysisFyWindow,
  ChatMessage,
  DesktopSettings,
  QuickAnalysis,
} from '@/lib/types'

const messageId = () => crypto.randomUUID()
const CONNECTION_CHECK_INTERVAL_MS = 30_000

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'Ismeretlen hiba történt.'
}

export const useErpChat = () => {
  const [input, setInput] = useState('')
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [databaseStatus, setDatabaseStatus] = useState<
    'checking' | 'online' | 'offline'
  >('checking')
  const [lastDatabaseCheckAt, setLastDatabaseCheckAt] = useState<number | null>(
    null,
  )
  const connectionCheckInFlight = useRef(false)
  const hasCompletedConnectionCheck = useRef(false)
  const [status, setStatus] = useState<'ready' | 'submitted'>('ready')
  const [error, setError] = useState<string | null>(null)
  const [settings, setSettings] = useState<DesktopSettings | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [availableModels, setAvailableModels] = useState<string[]>([])
  const [modelsLoading, setModelsLoading] = useState(true)

  const refreshModels = useCallback(async (currentModel: string) => {
    setModelsLoading(true)
    try {
      const models = await listAiModels()
      setAvailableModels(
        Array.from(new Set([currentModel, ...models])).filter(Boolean),
      )
    } catch {
      setAvailableModels([currentModel])
    } finally {
      setModelsLoading(false)
    }
  }, [])

  const refreshConnection = useCallback(async () => {
    if (connectionCheckInFlight.current) return

    connectionCheckInFlight.current = true
    if (!hasCompletedConnectionCheck.current) {
      setDatabaseStatus('checking')
    }

    try {
      await checkDatabase()
      setDatabaseStatus('online')
    } catch {
      setDatabaseStatus('offline')
    } finally {
      setLastDatabaseCheckAt(Date.now())
      hasCompletedConnectionCheck.current = true
      connectionCheckInFlight.current = false
    }
  }, [])

  const refreshSettings = useCallback(async () => {
    try {
      const loaded = await loadSettings()
      setSettings(loaded)
      setSettingsOpen(!loaded.hasMysqlPassword || !loaded.hasAiApiKey)
      void refreshModels(loaded.aiModel)

      if (loaded.hasMysqlPassword) {
        await refreshConnection()
      } else {
        setDatabaseStatus('offline')
      }
    } catch (loadError) {
      setError(errorText(loadError))
      setDatabaseStatus('offline')
    }
  }, [refreshConnection, refreshModels])

  useEffect(() => {
    let active = true

    void loadSettings()
      .then(async (loaded) => {
        if (!active) return
        setSettings(loaded)
        setSettingsOpen(!loaded.hasMysqlPassword || !loaded.hasAiApiKey)
        void refreshModels(loaded.aiModel)

        if (!loaded.hasMysqlPassword) {
          setDatabaseStatus('offline')
          return
        }

        await refreshConnection()
      })
      .catch((loadError: unknown) => {
        if (!active) return
        setError(errorText(loadError))
        setDatabaseStatus('offline')
      })

    return () => {
      active = false
    }
  }, [refreshConnection, refreshModels])

  useEffect(() => {
    if (!settings?.hasMysqlPassword) return

    const checkWhenVisible = () => {
      if (document.visibilityState === 'visible') {
        void refreshConnection()
      }
    }
    const interval = window.setInterval(
      checkWhenVisible,
      CONNECTION_CHECK_INTERVAL_MS,
    )
    window.addEventListener('focus', checkWhenVisible)
    document.addEventListener('visibilitychange', checkWhenVisible)

    return () => {
      window.clearInterval(interval)
      window.removeEventListener('focus', checkWhenVisible)
      document.removeEventListener('visibilitychange', checkWhenVisible)
    }
  }, [refreshConnection, settings?.hasMysqlPassword])

  const changeModel = async (model: string) => {
    if (!settings || model === settings.aiModel || status !== 'ready') return

    const previous = settings
    setError(null)
    setSettings({ ...settings, aiModel: model })

    try {
      const saved = await selectAiModel(model)
      setSettings(saved)
    } catch (modelError) {
      setSettings(previous)
      setError(errorText(modelError))
    }
  }

  const changeAnalysisFyWindow = async (years: AnalysisFyWindow) => {
    if (
      !settings ||
      years === settings.analysisFyWindow ||
      status !== 'ready'
    ) {
      return
    }

    const previous = settings
    setError(null)
    setSettings({ ...settings, analysisFyWindow: years })

    try {
      const saved = await selectAnalysisFyWindow(years)
      setSettings(saved)
    } catch (windowError) {
      setSettings(previous)
      setError(errorText(windowError))
    }
  }

  const submitMessage = async (
    content = input,
    quickAnalysis?: QuickAnalysis,
  ) => {
    const trimmedContent = content.trim()
    if (!trimmedContent || status !== 'ready') return

    if (!settings?.hasMysqlPassword || !settings.hasAiApiKey) {
      setSettingsOpen(true)
      setError('Előbb add meg az adatbázis- és AI-hozzáférést.')
      return
    }

    const userMessage: ChatMessage = {
      id: messageId(),
      role: 'user',
      content: trimmedContent,
    }
    const previousMessages = messages
    setInput('')
    setError(null)
    setMessages((current) => [...current, userMessage])
    setStatus('submitted')

    try {
      const response = quickAnalysis
        ? await analyzeQuickErp(quickAnalysis, trimmedContent)
        : await analyzeErp(trimmedContent, previousMessages)
      setMessages((current) => [
        ...current,
        {
          id: messageId(),
          role: 'assistant',
          content: response.summary,
          result: response.result,
        },
      ])
      setDatabaseStatus('online')
    } catch (submitError) {
      setError(errorText(submitError))
    } finally {
      setStatus('ready')
    }
  }

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    void submitMessage()
  }

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault()
      void submitMessage()
    }
  }

  const clearConversation = () => {
    setMessages([])
    setInput('')
    setError(null)
  }

  return {
    clearConversation,
    availableModels,
    changeAnalysisFyWindow,
    changeModel,
    databaseStatus,
    error,
    handleKeyDown,
    handleSubmit,
    input,
    lastDatabaseCheckAt,
    messages,
    modelsLoading,
    refreshConnection,
    refreshSettings,
    setInput,
    setSettingsOpen,
    settings,
    settingsOpen,
    status,
    submitMessage,
  }
}
