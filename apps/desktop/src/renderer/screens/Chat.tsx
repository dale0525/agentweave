import { FormEvent, useRef, useState } from "react";
import { Menu, Settings } from "lucide-react";

import {
  createServerSession,
  extractAssistantText,
  postSessionMessage
} from "../api";
import { AppIconButton } from "../components/AppIconButton";
import { Composer } from "../components/Composer";
import { ConversationDrawer } from "../components/ConversationDrawer";
import { MessageList } from "../components/MessageList";
import { starterMessages } from "../data/fixtures";
import { ChatMessage } from "../types";

type ChatProps = {
  onOpenSettings?: () => void;
};

function createMessageId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }

  return `message-${Math.random().toString(36).slice(2)}`;
}

export function Chat({
  onOpenSettings = () => undefined
}: ChatProps): JSX.Element {
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>(starterMessages);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [apiError, setApiError] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [isDrawerOpen, setIsDrawerOpen] = useState(false);
  const requestGenerationRef = useRef(0);

  const handleNewChat = () => {
    requestGenerationRef.current += 1;
    setMessages(starterMessages);
    setSessionId(null);
    setApiError(null);
    setIsSending(false);
    setIsDrawerOpen(false);
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const text = draft.trim();
    if (!text || isSending) {
      return;
    }

    setApiError(null);
    setMessages((current) => [
      ...current,
      {
        body: text,
        id: createMessageId(),
        role: "user"
      }
    ]);
    setDraft("");

    const requestGeneration = requestGenerationRef.current + 1;
    requestGenerationRef.current = requestGeneration;
    const isCurrentRequest = () => requestGenerationRef.current === requestGeneration;

    try {
      setIsSending(true);
      let activeSessionId = sessionId;
      if (!activeSessionId) {
        const session = await createServerSession("Provider adapter MVP");
        if (!isCurrentRequest()) {
          return;
        }
        activeSessionId = session.id;
        setSessionId(session.id);
      }

      const response = await postSessionMessage(activeSessionId, text);
      if (!isCurrentRequest()) {
        return;
      }
      const assistantText = extractAssistantText(response);
      if (assistantText) {
        setMessages((current) => [
          ...current,
          {
            body: assistantText,
            id: createMessageId(),
            role: "assistant"
          }
        ]);
      }
    } catch {
      if (isCurrentRequest()) {
        setApiError("Could not send message. Check your model or service connection.");
      }
    } finally {
      if (isCurrentRequest()) {
        setIsSending(false);
      }
    }
  };

  return (
    <main className="chat-shell" aria-label="GeneralAgent chat">
      <ConversationDrawer
        isOpen={isDrawerOpen}
        onNewChat={handleNewChat}
        onOpenChange={setIsDrawerOpen}
      />
      <header className="top-bar chat-top-bar">
        <AppIconButton
          label="Open conversations"
          onClick={() => setIsDrawerOpen(true)}
        >
          <Menu size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>GeneralAgent</h1>
          <p>Ask naturally. The agent will handle the work.</p>
        </div>
        <AppIconButton label="Open settings" onClick={onOpenSettings}>
          <Settings size={18} aria-hidden="true" />
        </AppIconButton>
      </header>
      <MessageList messages={messages} />
      <Composer
        draft={draft}
        error={apiError}
        isSending={isSending}
        onChange={setDraft}
        onSubmit={handleSubmit}
      />
    </main>
  );
}
