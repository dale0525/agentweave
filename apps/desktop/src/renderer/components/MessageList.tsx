import { ChatMessage } from "../types";

type MessageListProps = {
  messages: ChatMessage[];
};

export function MessageList({ messages }: MessageListProps): JSX.Element {
  return (
    <section className="message-list" aria-live="polite" aria-label="Conversation">
      {messages.map((message) => (
        <article
          className={`message-bubble message-bubble-${message.role}`}
          key={message.id}
        >
          <p>{message.body}</p>
        </article>
      ))}
    </section>
  );
}
