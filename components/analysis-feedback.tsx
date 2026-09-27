'use client'

import { Check, LoaderCircle, Mail, ThumbsDown, ThumbsUp } from 'lucide-react'
import { FormEvent, useState } from 'react'

import { emailAnalysisFeedback, saveAnalysisFeedback } from '@/lib/desktop'
import type { AnalysisFeedbackInput, ChatMessage } from '@/lib/types'

type AnalysisFeedbackProps = {
  message: ChatMessage
}

const categories: Array<{
  value: NonNullable<AnalysisFeedbackInput['category']>
  label: string
}> = [
  { value: 'entity', label: 'Rossz szervezet vagy végfelhasználó' },
  { value: 'metric', label: 'Hibás összeg vagy üzleti definíció' },
  { value: 'period', label: 'Rossz időszak' },
  { value: 'relationship', label: 'Hibás vagy hiányzó adatkapcsolat' },
  { value: 'missing-results', label: 'Hiányos találatok' },
  { value: 'summary', label: 'Félrevezető összefoglaló' },
  { value: 'visualization', label: 'Megjelenítési probléma' },
  { value: 'other', label: 'Egyéb' },
]

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'A visszajelzés mentése nem sikerült.'
}

const feedbackPayload = (
  message: ChatMessage,
  rating: AnalysisFeedbackInput['rating'],
  category?: AnalysisFeedbackInput['category'],
  correction = '',
): AnalysisFeedbackInput => ({
  rating,
  category,
  correction,
  question: message.question ?? 'A kapcsolódó kérdés nem érhető el.',
  answer: message.content,
  resultTitle: message.result?.title,
  sql: message.result?.sql,
  querySource: message.result?.querySource,
  rowCount: message.result?.rowCount,
})

export const AnalysisFeedback = ({ message }: AnalysisFeedbackProps) => {
  const [mode, setMode] = useState<'idle' | 'negative' | 'saved'>('idle')
  const [category, setCategory] =
    useState<NonNullable<AnalysisFeedbackInput['category']>>('other')
  const [correction, setCorrection] = useState('')
  const [feedbackId, setFeedbackId] = useState<string | null>(null)
  const [isSaving, setIsSaving] = useState(false)
  const [isEmailing, setIsEmailing] = useState(false)
  const [emailOpened, setEmailOpened] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const savePositive = async () => {
    if (isSaving || mode !== 'idle') return
    setIsSaving(true)
    setError(null)
    try {
      const id = await saveAnalysisFeedback(
        feedbackPayload(message, 'positive'),
      )
      setFeedbackId(id)
      setMode('saved')
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setIsSaving(false)
    }
  }

  const saveNegative = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!correction.trim() || isSaving) return
    setIsSaving(true)
    setError(null)
    try {
      const id = await saveAnalysisFeedback(
        feedbackPayload(message, 'negative', category, correction.trim()),
      )
      setFeedbackId(id)
      setMode('saved')
    } catch (saveError) {
      setError(errorText(saveError))
    } finally {
      setIsSaving(false)
    }
  }

  const openEmail = async () => {
    if (!feedbackId || isEmailing) return
    setIsEmailing(true)
    setError(null)
    try {
      await emailAnalysisFeedback(feedbackId)
      setEmailOpened(true)
    } catch (emailError) {
      setError(errorText(emailError))
    } finally {
      setIsEmailing(false)
    }
  }

  return (
    <section className="analysis-feedback" aria-label="Válasz értékelése">
      {mode === 'idle' ? (
        <div className="analysis-feedback-question">
          <span>Hasznos és helyes volt a válasz?</span>
          <button
            aria-label="A válasz helyes"
            disabled={isSaving}
            onClick={() => void savePositive()}
            title="Helyes"
            type="button"
          >
            {isSaving ? (
              <LoaderCircle aria-hidden="true" className="spin" size={14} />
            ) : (
              <ThumbsUp aria-hidden="true" size={14} />
            )}
          </button>
          <button
            aria-label="A válasz javítandó"
            disabled={isSaving}
            onClick={() => setMode('negative')}
            title="Javítandó"
            type="button"
          >
            <ThumbsDown aria-hidden="true" size={14} />
          </button>
        </div>
      ) : null}

      {mode === 'negative' ? (
        <form className="analysis-feedback-form" onSubmit={saveNegative}>
          <div>
            <label htmlFor={`feedback-category-${message.id}`}>Mi hibás?</label>
            <select
              id={`feedback-category-${message.id}`}
              value={category}
              onChange={(event) =>
                setCategory(
                  event.target.value as NonNullable<
                    AnalysisFeedbackInput['category']
                  >,
                )
              }
            >
              {categories.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </div>
          <label htmlFor={`feedback-correction-${message.id}`}>
            Hogyan lenne helyes?
          </label>
          <textarea
            autoFocus
            id={`feedback-correction-${message.id}`}
            maxLength={4000}
            placeholder="Például: az AERONAUTICA nem tartozik az AERON-hoz…"
            required
            rows={3}
            value={correction}
            onChange={(event) => setCorrection(event.target.value)}
          />
          <div className="analysis-feedback-actions">
            <button onClick={() => setMode('idle')} type="button">
              Mégse
            </button>
            <button disabled={!correction.trim() || isSaving} type="submit">
              {isSaving ? (
                <LoaderCircle aria-hidden="true" className="spin" size={14} />
              ) : (
                <Check aria-hidden="true" size={14} />
              )}
              Javítás mentése
            </button>
          </div>
        </form>
      ) : null}

      {mode === 'saved' ? (
        <div className="analysis-feedback-saved">
          <Check aria-hidden="true" size={14} />
          <span>
            {correction
              ? 'A javítás bekerült a helyi fejlesztési várólistába.'
              : 'Köszönöm, a helyes választ rögzítettem.'}
          </span>
          {correction ? (
            <button
              disabled={isEmailing || emailOpened}
              onClick={() => void openEmail()}
              type="button"
            >
              {isEmailing ? (
                <LoaderCircle aria-hidden="true" className="spin" size={13} />
              ) : (
                <Mail aria-hidden="true" size={13} />
              )}
              {emailOpened ? 'Levél előkészítve' : 'Küldés Mártonnak'}
            </button>
          ) : null}
        </div>
      ) : null}
      {error ? <p className="analysis-feedback-error">{error}</p> : null}
    </section>
  )
}
