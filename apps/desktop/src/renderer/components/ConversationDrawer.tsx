import * as Dialog from "@radix-ui/react-dialog";
import { Plus, Search, X } from "lucide-react";

import { conversations } from "../data/fixtures";
import { AppIconButton } from "./AppIconButton";
import { useI18n } from "../i18n/I18nProvider";

type ConversationDrawerProps = {
  isOpen: boolean;
  onNewChat: () => void;
  onOpenChange: (isOpen: boolean) => void;
};

function getSectionKey(updatedAt: string): string {
  if (updatedAt === "Just now" || updatedAt.includes("ago")) {
    return "conversation.today";
  }

  if (updatedAt === "Yesterday") {
    return "conversation.yesterday";
  }

  return "conversation.previous30Days";
}

export function ConversationDrawer({
  isOpen,
  onNewChat,
  onOpenChange
}: ConversationDrawerProps): JSX.Element {
  const { t } = useI18n();
  let currentSection = "";

  return (
    <Dialog.Root open={isOpen} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="conversation-drawer-overlay" />
        <Dialog.Content
          aria-label={t("conversation.title")}
          className="conversation-drawer-content"
        >
          <header className="conversation-drawer-header">
            <Dialog.Title>{t("conversation.title")}</Dialog.Title>
            <AppIconButton label={t("conversation.close")} onClick={() => onOpenChange(false)}>
              <X size={16} aria-hidden="true" />
            </AppIconButton>
          </header>

          <button className="conversation-drawer-new" onClick={onNewChat} type="button">
            <Plus size={14} aria-hidden="true" />
            <span>{t("conversation.new")}</span>
          </button>

          <label className="search-box conversation-drawer-search">
            <Search size={14} aria-hidden="true" />
            <span className="sr-only">{t("conversation.search")}</span>
            <input type="search" placeholder={t("conversation.search")} />
          </label>

          <div className="conversation-drawer-list">
            {conversations.map((conversation) => {
              const sectionLabel = getSectionKey(conversation.updatedAt);
              const shouldShowSection = sectionLabel !== currentSection;
              currentSection = sectionLabel;

              return (
                <div key={conversation.id}>
                  {shouldShowSection ? (
                    <p className="section-label conversation-drawer-section">
                      {t(sectionLabel)}
                    </p>
                  ) : null}
                  <button className="conversation-row" type="button">
                    <span>
                      <strong>{conversationTitle(conversation.id, conversation.title, t)}</strong>
                      <small>{conversationUpdatedAt(conversation.updatedAt, t)}</small>
                    </span>
                  </button>
                </div>
              );
            })}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function conversationTitle(
  id: string,
  fallback: string,
  t: ReturnType<typeof useI18n>["t"]
): string {
  const key = {
    new: "conversation.fixtureNew",
    trip: "conversation.fixtureTrip",
    draft: "conversation.fixtureDraft",
    research: "conversation.fixtureResearch"
  }[id];
  return key ? t(key) : fallback;
}

function conversationUpdatedAt(
  value: string,
  t: ReturnType<typeof useI18n>["t"]
): string {
  if (value === "Just now") return t("conversation.justNow");
  if (value === "Yesterday") return t("conversation.yesterday");
  const hours = /^(\d+) hours ago$/.exec(value)?.[1];
  return hours ? t("conversation.hoursAgo", { count: hours }) : value;
}
