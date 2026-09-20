'use client'

import {
  FormEvent,
  KeyboardEvent,
  useCallback,
  useEffect,
  useState,
} from 'react'

import {
  analyzeErp,
  checkDatabase,
  listAiModels,
  loadSettings,
  selectAiModel,
} from '@/lib/desktop'
import type { ChatMessage, DesktopSettings } from '@/lib/types'

const messageId = () => crypto.randomUUID()

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
    setDatabaseStatus('checking')

    try {
      await checkDatabase()
      setDatabaseStatus('online')
    } catch {
      setDatabaseStatus('offline')
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

        try {
          await checkDatabase()
          if (active) setDatabaseStatus('online')
        } catch {
          if (active) setDatabaseStatus('offline')
        }
      })
      .catch((loadError: unknown) => {
        if (!active) return
        setError(errorText(loadError))
        setDatabaseStatus('offline')
      })

    return () => {
      active = false
    }
  }, [refreshModels])

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

  const submitMessage = async (content = input) => {
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
      const response = await analyzeErp(trimmedContent, previousMessages)
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
    changeModel,
    databaseStatus,
    error,
    handleKeyDown,
    handleSubmit,
    input,
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
