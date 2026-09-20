import { invoke } from '@tauri-apps/api/core'

import type {
  AnalyzeResponse,
  ChatMessage,
  DesktopSettings,
  SettingsInput,
} from '@/lib/types'

const ensureDesktop = () => {
  const tauriWindow = window as Window & { __TAURI_INTERNALS__?: unknown }

  if (!tauriWindow.__TAURI_INTERNALS__) {
    throw new Error(
      'Az élő adatkapcsolat csak a telepített ERGO asztali alkalmazásban érhető el.',
    )
  }
}

export const loadSettings = async (): Promise<DesktopSettings> => {
  ensureDesktop()
  return invoke<DesktopSettings>('load_settings')
}

export const saveSettings = async (
  settings: SettingsInput,
): Promise<DesktopSettings> => {
  ensureDesktop()
  return invoke<DesktopSettings>('save_settings', { settings })
}

export const checkDatabase = async (): Promise<void> => {
  ensureDesktop()
  await invoke('check_database')
}

export const listAiModels = async (): Promise<string[]> => {
  ensureDesktop()
  return invoke<string[]>('list_ai_models')
}

export const selectAiModel = async (
  model: string,
): Promise<DesktopSettings> => {
  ensureDesktop()
  return invoke<DesktopSettings>('select_ai_model', { model })
}

export const analyzeErp = async (
  question: string,
  messages: ChatMessage[],
): Promise<AnalyzeResponse> => {
  ensureDesktop()
  return invoke<AnalyzeResponse>('analyze_erp', {
    question,
    history: messages.slice(-8).map(({ content, role }) => ({ content, role })),
  })
}
