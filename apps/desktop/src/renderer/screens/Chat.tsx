import { FormEvent, useEffect, useRef, useState } from "react";
import { Menu, Settings } from "lucide-react";

import {
  createServerSession,
  postSessionMessage
} from "../api";
import { buildAssistantTurnMessages } from "../chatEventMessages";
import { AppIconButton } from "../components/AppIconButton";
import { Composer } from "../components/Composer";
import { ConversationDrawer } from "../components/ConversationDrawer";
import { MessageList } from "../components/MessageList";
import { starterMessages } from "../data/fixtures";
import { loadSavedModelSettings } from "../modelSettings";
import { ChatMessage } from "../types";
import { useI18n } from "../i18n/I18nProvider";

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
  const { t } = useI18n();
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>(starterMessages);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [apiError, setApiError] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [isDrawerOpen, setIsDrawerOpen] = useState(false);
  const requestGenerationRef = useRef(0);

  useEffect(() => {
    setMessages((current) => current.map((message) => (
      message.id === "starter-assistant" ? { ...message, body: t("chat.starter") } : message
    )));
  }, [t]);

  const handleNewChat = () => {
    requestGenerationRef.current += 1;
    setMessages(starterMessages.map((message) => ({ ...message, body: t("chat.starter") })));
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

    const pendingReasoningId = createMessageId();
    setApiError(null);
    setMessages((current) => [
      ...current,
      {
        body: text,
        id: createMessageId(),
        role: "user"
      },
      {
        id: pendingReasoningId,
        kind: "reasoning",
        role: "assistant",
        status: "running",
        text: t("chat.working")
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

      const response = await postSessionMessage(
        activeSessionId,
        text,
        await loadSavedModelSettings()
      );
      if (!isCurrentRequest()) {
        return;
      }
      const assistantMessages = buildAssistantTurnMessages(response, createMessageId);
      setMessages((current) => [
        ...current.filter((message) => message.id !== pendingReasoningId),
        ...assistantMessages
      ]);
    } catch {
      if (isCurrentRequest()) {
        setMessages((current) =>
          current.filter((message) => message.id !== pendingReasoningId)
        );
        setApiError(t("chat.sendError"));
      }
    } finally {
      if (isCurrentRequest()) {
        setIsSending(false);
      }
    }
  };

  return (
    <main className="chat-shell" aria-label={t("chat.ariaLabel")}>
      <ConversationDrawer
        isOpen={isDrawerOpen}
        onNewChat={handleNewChat}
        onOpenChange={setIsDrawerOpen}
      />
      <header className="top-bar chat-top-bar">
        <AppIconButton
          label={t("chat.openConversations")}
          onClick={() => setIsDrawerOpen(true)}
        >
          <Menu size={18} aria-hidden="true" />
        </AppIconButton>
        <div className="top-bar-title">
          <h1>{t("app.name")}</h1>
          <p>{t("app.tagline")}</p>
        </div>
        <AppIconButton label={t("chat.openSettings")} onClick={onOpenSettings}>
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
