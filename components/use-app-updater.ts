'use client'

import type { Update } from '@tauri-apps/plugin-updater'
import { useCallback, useEffect, useRef, useState } from 'react'

export type AppUpdateState = {
  status:
    | 'idle'
    | 'checking'
    | 'current'
    | 'available'
    | 'downloading'
    | 'installing'
    | 'error'
  currentVersion: string | null
  availableVersion: string | null
  notes: string | null
  downloadedBytes: number
  totalBytes: number | null
  error: string | null
}

const initialState: AppUpdateState = {
  status: 'idle',
  currentVersion: null,
  availableVersion: null,
  notes: null,
  downloadedBytes: 0,
  totalBytes: null,
  error: null,
}

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'A frissítések ellenőrzése nem sikerült.'
}

const isDesktopApp = () =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export const useAppUpdater = () => {
  const [state, setState] = useState<AppUpdateState>(initialState)
  const updateRef = useRef<Update | null>(null)

  const checkForUpdate = useCallback(async (silent = false) => {
    if (!isDesktopApp()) return

    setState((current) => ({
      ...current,
      status: 'checking',
      error: null,
    }))

    try {
      const [{ getVersion }, { check }] = await Promise.all([
        import('@tauri-apps/api/app'),
        import('@tauri-apps/plugin-updater'),
      ])
      const [versionResult, updateResult] = await Promise.allSettled([
        getVersion(),
        check({ timeout: 20_000 }),
      ])
      const currentVersion =
        versionResult.status === 'fulfilled' ? versionResult.value : null

      if (updateResult.status === 'rejected') {
        setState((current) => ({ ...current, currentVersion }))
        throw updateResult.reason
      }

      const update = updateResult.value

      if (updateRef.current && updateRef.current !== update) {
        await updateRef.current.close().catch(() => undefined)
      }
      updateRef.current = update

      setState({
        status: update ? 'available' : 'current',
        currentVersion,
        availableVersion: update?.version ?? null,
        notes: update?.body?.trim() || null,
        downloadedBytes: 0,
        totalBytes: null,
        error: null,
      })
    } catch (error) {
      setState((current) => ({
        ...current,
        status: silent ? 'idle' : 'error',
        error: silent ? null : errorText(error),
      }))
    }
  }, [])

  const installUpdate = useCallback(async () => {
    const update = updateRef.current
    if (!update || state.status !== 'available') return

    setState((current) => ({
      ...current,
      status: 'downloading',
      downloadedBytes: 0,
      totalBytes: null,
      error: null,
    }))

    try {
      await update.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          setState((current) => ({
            ...current,
            totalBytes: event.data.contentLength ?? null,
          }))
          return
        }

        if (event.event === 'Progress') {
          setState((current) => ({
            ...current,
            downloadedBytes: current.downloadedBytes + event.data.chunkLength,
          }))
          return
        }

        setState((current) => ({ ...current, status: 'installing' }))
      })

      const { relaunch } = await import('@tauri-apps/plugin-process')
      await relaunch()
    } catch (error) {
      setState((current) => ({
        ...current,
        status: 'error',
        error: errorText(error),
      }))
    }
  }, [state.status])

  useEffect(() => {
    void checkForUpdate(true)

    return () => {
      const update = updateRef.current
      updateRef.current = null
      if (update) void update.close().catch(() => undefined)
    }
  }, [checkForUpdate])

  return {
    checkForUpdate,
    installUpdate,
    state,
  }
}
