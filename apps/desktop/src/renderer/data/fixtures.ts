import { ChatMessage, ConversationSummary, SkillSummary } from "../types";

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

export const skills: SkillSummary[] = [
  {
    description: "Quickly read and organize your uploaded files.",
    enabled: true,
    id: "file-helper",
    name: "File Helper",
    status: "active"
  },
  {
    description: "Browse the web to find the latest information and answers.",
    enabled: true,
    id: "web-research",
    name: "Web Research",
    status: "active"
  },
  {
    description: "Schedule events and check your availability.",
    enabled: false,
    id: "calendar",
    name: "Calendar",
    status: "inactive"
  },
  {
    description: "Run local actions when the desktop bridge supports them.",
    enabled: false,
    id: "local-command",
    name: "Local Command",
    status: "unavailable"
  }
];
