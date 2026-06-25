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
