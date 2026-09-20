import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

import { QueryResultCard } from '@/components/query-result-card'
import type { ChatMessage } from '@/lib/types'

type MessageViewProps = {
  message: ChatMessage
}

export const MessageView = ({ message }: MessageViewProps) => (
  <article className={`message-row ${message.role}`}>
    <div className="message-avatar">{message.role === 'user' ? 'Ön' : 'E'}</div>
    <div className="message-body">
      <p className="message-author">
        {message.role === 'user' ? 'Ön' : 'Ergo elemző'}
      </p>
      <div className="message-parts">
        <div className="markdown">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>
            {message.content}
          </ReactMarkdown>
        </div>
        {message.result ? <QueryResultCard result={message.result} /> : null}
      </div>
    </div>
  </article>
)
