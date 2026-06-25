import * as Dialog from "@radix-ui/react-dialog";
import { Plus, Search, X } from "lucide-react";

import { conversations } from "../data/fixtures";
import { AppIconButton } from "./AppIconButton";

type ConversationDrawerProps = {
  isOpen: boolean;
  onNewChat: () => void;
  onOpenChange: (isOpen: boolean) => void;
};

function getSectionLabel(updatedAt: string): string {
  if (updatedAt === "Just now" || updatedAt.includes("ago")) {
    return "Today";
  }

  if (updatedAt === "Yesterday") {
    return "Yesterday";
  }

  return "Previous 30 days";
}

export function ConversationDrawer({
  isOpen,
  onNewChat,
  onOpenChange
}: ConversationDrawerProps): JSX.Element {
  let currentSection = "";

  return (
    <Dialog.Root open={isOpen} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="conversation-drawer-overlay" />
        <Dialog.Content
          aria-label="Conversations"
          className="conversation-drawer-content"
        >
          <header className="conversation-drawer-header">
            <Dialog.Title>Conversations</Dialog.Title>
            <AppIconButton label="Close conversations" onClick={() => onOpenChange(false)}>
              <X size={16} aria-hidden="true" />
            </AppIconButton>
          </header>

          <button className="conversation-drawer-new" onClick={onNewChat} type="button">
            <Plus size={14} aria-hidden="true" />
            <span>New chat</span>
          </button>

          <label className="search-box conversation-drawer-search">
            <Search size={14} aria-hidden="true" />
            <span className="sr-only">Search conversations</span>
            <input type="search" placeholder="Search conversations" />
          </label>

          <div className="conversation-drawer-list">
            {conversations.map((conversation) => {
              const sectionLabel = getSectionLabel(conversation.updatedAt);
              const shouldShowSection = sectionLabel !== currentSection;
              currentSection = sectionLabel;

              return (
                <div key={conversation.id}>
                  {shouldShowSection ? (
                    <p className="section-label conversation-drawer-section">
                      {sectionLabel}
                    </p>
                  ) : null}
                  <button className="session-row" type="button">
                    <span>
                      <strong>{conversation.title}</strong>
                      <small>{conversation.updatedAt}</small>
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
