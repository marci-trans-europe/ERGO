'use client'

import { FormEvent, useState } from 'react'
import { LoaderCircle, MessageCircle, Send, X } from 'lucide-react'

import { sendFeedback } from '@/lib/desktop'

const errorText = (error: unknown): string => {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return 'A levelezőalkalmazás nem nyitható meg.'
}

export const FeedbackWidget = () => {
  const [isOpen, setIsOpen] = useState(false)
  const [message, setMessage] = useState('')
  const [status, setStatus] = useState<'idle' | 'opening' | 'opened'>('idle')
  const [error, setError] = useState<string | null>(null)

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const trimmed = message.trim()
    if (!trimmed || status === 'opening') return

    setError(null)
    setStatus('opening')
    try {
      await sendFeedback(trimmed)
      setStatus('opened')
      setMessage('')
    } catch (feedbackError) {
      setError(errorText(feedbackError))
      setStatus('idle')
    }
  }

  return (
    <aside className="feedback-widget" aria-label="Visszajelzés küldése">
      {isOpen ? (
        <section className="feedback-panel" aria-labelledby="feedback-title">
          <header>
            <div>
              <p className="eyebrow accent">VISSZAJELZÉS</p>
              <h2 id="feedback-title">Írj Mártonnak</h2>
            </div>
            <button
              aria-label="Visszajelzés bezárása"
              onClick={() => setIsOpen(false)}
              type="button"
            >
              <X aria-hidden="true" size={17} />
            </button>
          </header>
          <div className="feedback-conversation">
            <p>
              Írd meg röviden, mi nem úgy működött, ahogyan vártad. Az ERGO
              előkészít egy e-mailt a géped levelezőprogramjában; elküldés előtt
              még ellenőrizheted.
            </p>
            {status === 'opened' ? (
              <p className="feedback-success">
                Megnyitottam a levelet. A levelezőprogramban tudod elküldeni.
              </p>
            ) : null}
          </div>
          <form onSubmit={handleSubmit}>
            <label htmlFor="feedback-message">Mi történt?</label>
            <textarea
              autoFocus
              id="feedback-message"
              maxLength={2000}
              placeholder="Például: a mai számlák lekérdezése üres eredményt adott…"
              rows={5}
              value={message}
              onChange={(event) => {
                setMessage(event.target.value)
                if (status === 'opened') setStatus('idle')
              }}
            />
            {error ? <p className="feedback-error">{error}</p> : null}
            <button disabled={!message.trim() || status === 'opening'} type="submit">
              {status === 'opening' ? (
                <LoaderCircle aria-hidden="true" className="spin" size={16} />
              ) : (
                <Send aria-hidden="true" size={16} />
              )}
              Folytatás a Mailben
            </button>
          </form>
        </section>
      ) : null}
      <button
        aria-expanded={isOpen}
        aria-label={isOpen ? 'Visszajelzés bezárása' : 'Visszajelzés küldése'}
        className="feedback-trigger"
        onClick={() => setIsOpen((current) => !current)}
        type="button"
      >
        {isOpen ? (
          <X aria-hidden="true" size={20} />
        ) : (
          <MessageCircle aria-hidden="true" size={20} />
        )}
        <span>{isOpen ? 'Bezárás' : 'Visszajelzés'}</span>
      </button>
    </aside>
  )
}
