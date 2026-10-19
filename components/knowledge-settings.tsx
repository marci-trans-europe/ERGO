'use client'

import {
  BookOpenCheck,
  Check,
  LoaderCircle,
  Mail,
  RefreshCw,
  X,
} from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'

import {
  approveLearnedAlias,
  emailAnalysisFeedback,
  loadKnowledgeOverview,
  loadLearnedAliases,
  rejectLearnedAlias,
} from '@/lib/desktop'
import type { KnowledgeOverview, LearnedAlias } from '@/lib/types'

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

const roleLabels: Record<LearnedAlias['role'], string> = {
  end_customer: 'Végfelhasználó',
  billed_customer: 'Számlázott vevő',
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
  const [aliases, setAliases] = useState<LearnedAlias[]>([])
  const [selectedCodes, setSelectedCodes] = useState<Record<string, string[]>>(
    {},
  )
  const [aliasBusyId, setAliasBusyId] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [loadedOverview, loadedAliases] = await Promise.all([
        loadKnowledgeOverview(),
        loadLearnedAliases(),
      ])
      setOverview(loadedOverview)
      setAliases(loadedAliases)
    } catch (loadError) {
      setError(errorText(loadError))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    let active = true
    void loadLearnedAliases()
      .then((loaded) => {
        if (active) setAliases(loaded)
      })
      .catch(() => {
        // Az aliastár hiánya ne akadályozza a tudásállapot megjelenítését.
      })
    return () => {
      active = false
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

  const toggleCode = (aliasId: string, code: string) => {
    setSelectedCodes((current) => {
      const selected = current[aliasId] ?? []
      return {
        ...current,
        [aliasId]: selected.includes(code)
          ? selected.filter((entry) => entry !== code)
          : [...selected, code],
      }
    })
  }

  const approveAlias = async (aliasId: string) => {
    const codes = selectedCodes[aliasId] ?? []
    if (!codes.length) {
      setError('Jelöld ki, melyik cég a helyes, mielőtt jóváhagyod.')
      return
    }
    setAliasBusyId(aliasId)
    setError(null)
    try {
      setAliases(await approveLearnedAlias(aliasId, codes))
    } catch (approveError) {
      setError(errorText(approveError))
    } finally {
      setAliasBusyId(null)
    }
  }

  const rejectAlias = async (aliasId: string) => {
    setAliasBusyId(aliasId)
    setError(null)
    try {
      setAliases(await rejectLearnedAlias(aliasId))
    } catch (rejectError) {
      setError(errorText(rejectError))
    } finally {
      setAliasBusyId(null)
    }
  }

  const pendingAliases = aliases.filter((alias) => alias.status === 'pending')
  const approvedAliases = aliases.filter((alias) => alias.status === 'approved')

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
      <div className="knowledge-alias-block">
        <strong className="knowledge-alias-title">
          Jóváhagyásra váró cégazonosítás
        </strong>
        {pendingAliases.length ? (
          <div className="knowledge-alias-list">
            {pendingAliases.map((alias) => {
              const selected = selectedCodes[alias.id] ?? []
              return (
                <article key={alias.id}>
                  <div>
                    <span className="knowledge-feedback-category">
                      {roleLabels[alias.role]}
                    </span>
                    <time>
                      {new Date(alias.createdAt).toLocaleDateString('hu-HU')}
                    </time>
                  </div>
                  <strong>„{alias.searchName}” — melyik a helyes?</strong>
                  <ul className="knowledge-alias-candidates">
                    {alias.candidates.map((candidate) => (
                      <li key={candidate.code}>
                        <label>
                          <input
                            checked={selected.includes(candidate.code)}
                            onChange={() =>
                              toggleCode(alias.id, candidate.code)
                            }
                            type="checkbox"
                          />
                          <span>
                            {candidate.label} <code>{candidate.code}</code>
                          </span>
                        </label>
                      </li>
                    ))}
                  </ul>
                  <div className="knowledge-alias-actions">
                    <button
                      disabled={aliasBusyId === alias.id || !selected.length}
                      onClick={() => void approveAlias(alias.id)}
                      type="button"
                    >
                      {aliasBusyId === alias.id ? (
                        <LoaderCircle
                          aria-hidden="true"
                          className="spin"
                          size={13}
                        />
                      ) : (
                        <Check aria-hidden="true" size={13} />
                      )}
                      Jóváhagyás
                    </button>
                    <button
                      disabled={aliasBusyId === alias.id}
                      onClick={() => void rejectAlias(alias.id)}
                      type="button"
                    >
                      <X aria-hidden="true" size={13} />
                      Elvetés
                    </button>
                  </div>
                </article>
              )
            })}
          </div>
        ) : (
          <p className="knowledge-empty">
            Nincs jóváhagyásra váró cégazonosítás. Ha egy kérdésnél több cég is
            illik a névre, az ERGO itt fogja felajánlani.
          </p>
        )}
        {approvedAliases.length ? (
          <div className="knowledge-alias-approved">
            <span>Érvényben lévő azonosítások:</span>
            <ul>
              {approvedAliases.map((alias) => (
                <li key={alias.id}>
                  <span>
                    „{alias.searchName}” → {alias.label} (
                    {alias.codes.join(', ')}) · {roleLabels[alias.role]}
                  </span>
                  <button
                    disabled={aliasBusyId === alias.id}
                    onClick={() => void rejectAlias(alias.id)}
                    type="button"
                  >
                    Visszavonás
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
      </div>
      {error ? <p className="settings-error">{error}</p> : null}
      <p className="knowledge-note">
        A visszajelzések helyben maradnak. Nem írják át automatikusan az SQL-t
        vagy az üzleti szabályokat; a fejlesztési csomag ellenőrzés után
        kerülhet a következő tudásverzióba. A helyesnek jelölt lekérdezéseket és
        a javításaidat az ERGO hasonló kérdéseknél támpontként felhasználja. A
        cégazonosítás viszont pénzügyi számokat érint, ezért csak az itt
        jóváhagyott párosítás lép életbe.
      </p>
    </fieldset>
  )
}
