import { ChatMessage, ConversationSummary } from "../types";

export const starterMessages: ChatMessage[] = [
  {
    body: "Hello! How can I help you today?",
    id: "starter-assistant",
    role: "assistant"
  }
];

export const conversations: ConversationSummary[] = [
  { id: "new", title: "New conversation", updatedAt: "Just now" },
  { id: "trip", title: "Trip planning", updatedAt: "2 hours ago" },
  { id: "draft", title: "Draft reply", updatedAt: "Yesterday" },
  { id: "research", title: "Research notes", updatedAt: "Oct 24" }
];
