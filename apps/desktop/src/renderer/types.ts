export type ChatMessage = {
  body: string;
  id: string;
  role: "assistant" | "user";
};

export type ConversationSummary = {
  id: string;
  title: string;
  updatedAt: string;
};

export type EndpointType = "responses" | "chat_completions" | "completion";

export type ModelSettings = {
  apiKey: string;
  baseUrl: string;
  endpointType: EndpointType;
  modelName: string;
};

export type SkillStatus = "active" | "inactive" | "unavailable";

export type SkillSummary = {
  description: string;
  enabled: boolean;
  id: string;
  name: string;
  status: SkillStatus;
};
