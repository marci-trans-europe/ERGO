'use client'

import { BookOpenCheck, LoaderCircle, Mail, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'

import { emailAnalysisFeedback, loadKnowledgeOverview } from '@/lib/desktop'
import type { KnowledgeOverview } from '@/lib/types'

type KnowledgeSettingsProps = {
  initialOverview?: KnowledgeOverview
}

const categoryLabels: Record<string, string> = {
  entity: 'Szervezet',
  metric: 'Összeg / definíció',
  period: 'Időszak',
  relationship: 'Adatkapcsolat',
  'missing-results': 'Hiányos találat',
  summary: 'Összefoglaló',
  visualization: 'Megjelenítés',
  other: 'Egyéb',
}

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'A tudáscsomag állapota nem olvasható.'
}

export const KnowledgeSettings = ({
  initialOverview,
}: KnowledgeSettingsProps) => {
  const [overview, setOverview] = useState<KnowledgeOverview | null>(
    initialOverview ?? null,
  )
  const [loading, setLoading] = useState(!initialOverview)
  const [emailingId, setEmailingId] = useState<string | null>(null)
  const [sentIds, setSentIds] = useState<Set<string>>(() => new Set())
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setOverview(await loadKnowledgeOverview())
    } catch (loadError) {
      setError(errorText(loadError))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (initialOverview) return
    let active = true
    void loadKnowledgeOverview()
      .then((loaded) => {
        if (active) setOverview(loaded)
      })
      .catch((loadError: unknown) => {
        if (active) setError(errorText(loadError))
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
    }
  }, [initialOverview])

  const openEmail = async (feedbackId: string) => {
    setEmailingId(feedbackId)
    setError(null)
    try {
      await emailAnalysisFeedback(feedbackId)
      setSentIds((current) => new Set(current).add(feedbackId))
    } catch (emailError) {
      setError(errorText(emailError))
    } finally {
      setEmailingId(null)
    }
  }

  return (
    <fieldset className="knowledge-settings">
      <legend>
        <BookOpenCheck aria-hidden="true" size={17} /> Tudás és javítások
      </legend>
      {loading && !overview ? (
        <p className="knowledge-loading">
          <LoaderCircle aria-hidden="true" className="spin" size={15} />
          Helyi tudás betöltése…
        </p>
      ) : null}
      {overview ? (
        <>
          <div className="knowledge-summary">
            <div>
              <strong>Knowledge Pack {overview.version}</strong>
              <span>
                {overview.metricCount} mérőszám · {overview.relationshipCount}{' '}
                reláció · {overview.entityRuleCount} entitásszabály ·{' '}
                {overview.skillCount} skill
              </span>
            </div>
            <button
              aria-label="Tudásállapot frissítése"
              disabled={loading}
              onClick={() => void refresh()}
              title="Frissítés"
              type="button"
            >
              <RefreshCw
                aria-hidden="true"
                className={loading ? 'spin' : undefined}
                size={15}
              />
            </button>
          </div>
          <div className="knowledge-counters">
            <span>
              <strong>{overview.feedbackTotal}</strong> rögzített értékelés
            </span>
            <span className={overview.feedbackPending ? 'has-pending' : ''}>
              <strong>{overview.feedbackPending}</strong> fejlesztésre vár
            </span>
          </div>
          {overview.recentFeedback.length ? (
            <div className="knowledge-feedback-list">
              {overview.recentFeedback.map((feedback) => (
                <article key={feedback.id}>
                  <div>
                    <span className="knowledge-feedback-category">
                      {categoryLabels[feedback.category ?? 'other'] ?? 'Egyéb'}
                    </span>
                    <time>
                      {new Date(feedback.createdAt).toLocaleDateString('hu-HU')}
                    </time>
                  </div>
                  <strong>{feedback.question}</strong>
                  <p>{feedback.correction}</p>
                  <button
                    disabled={
                      emailingId === feedback.id || sentIds.has(feedback.id)
                    }
                    onClick={() => void openEmail(feedback.id)}
                    type="button"
                  >
                    {emailingId === feedback.id ? (
                      <LoaderCircle
                        aria-hidden="true"
                        className="spin"
                        size={13}
                      />
                    ) : (
                      <Mail aria-hidden="true" size={13} />
                    )}
                    {sentIds.has(feedback.id)
                      ? 'Levél előkészítve'
                      : 'Fejlesztési csomag'}
                  </button>
                </article>
              ))}
            </div>
          ) : (
            <p className="knowledge-empty">
              Még nincs javításra váró visszajelzés.
            </p>
          )}
        </>
      ) : null}
      {error ? <p className="settings-error">{error}</p> : null}
      <p className="knowledge-note">
        A visszajelzések helyben maradnak. Nem írják át automatikusan az SQL-t
        vagy az üzleti szabályokat; a fejlesztési csomag ellenőrzés után
        kerülhet a következő tudásverzióba.
      </p>
    </fieldset>
  )
}
