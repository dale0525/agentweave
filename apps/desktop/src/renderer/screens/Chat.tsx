import {
  Activity,
  Bot,
  FileText,
  History,
  Paperclip,
  Play,
  Plus,
  RefreshCw,
  Search,
  Send,
  Settings,
  Terminal,
  Wrench
} from "lucide-react";
import { FormEvent, ReactNode, useMemo, useState } from "react";

import {
  createServerSession,
  extractAssistantText,
  postSessionMessage
} from "../api";

type AppView = "chat" | "sessions";

type ChatProps = {
  onNavigate?: (view: AppView) => void;
};

type Message = {
  id: string;
  role: "agent" | "tool" | "user";
  body: string;
  meta: string;
};

const initialMessages: Message[] = [
  {
    id: "m-1",
    role: "user",
    body: "Initialize provider adapter for AWS.",
    meta: "18:42:01"
  },
  {
    id: "m-2",
    role: "agent",
    body: "Analyzing provider requirements...",
    meta: "system 0.4s"
  },
  {
    id: "m-3",
    role: "tool",
    body: 'echo_skill({"status":"init"})\n\n{"success": true}',
    meta: "tool call 18:42:02"
  },
  {
    id: "m-4",
    role: "agent",
    body: "Adapter initialized. Ready for deployment.",
    meta: "agent 18:42:03"
  }
];

const recentSessions = [
  { title: "Provider adapter MVP", state: "Running", time: "10:42 AM" },
  { title: "Schema validation fix", state: "Idle", time: "Yesterday" },
  { title: "API test", state: "Ready", time: "Oct 24" }
];

function createMessageId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }

  return `user-${Math.random().toString(36).slice(2)}`;
}

function IconButton({
  label,
  children,
  disabled = false,
  onClick,
  type = "button"
}: {
  label: string;
  children: ReactNode;
  disabled?: boolean;
  onClick?: () => void;
  type?: "button" | "submit";
}) {
  return (
    <button
      aria-label={label}
      className="icon-button"
      disabled={disabled}
      onClick={onClick}
      title={label}
      type={type}
    >
      {children}
    </button>
  );
}

export function Chat({ onNavigate = () => undefined }: ChatProps): JSX.Element {
  const [draft, setDraft] = useState("");
  const [messages, setMessages] = useState(initialMessages);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [apiError, setApiError] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);

  const tokenUsage = useMemo(() => {
    const added = messages.length - initialMessages.length;
    return 1240 + Math.max(added, 0) * 18;
  }, [messages.length]);

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
        id: createMessageId(),
        role: "user",
        body: text,
        meta: "you"
      }
    ]);
    setDraft("");

    try {
      setIsSending(true);
      let activeSessionId = sessionId;
      if (!activeSessionId) {
        const session = await createServerSession("Provider adapter MVP");
        activeSessionId = session.id;
        setSessionId(session.id);
      }

      const response = await postSessionMessage(activeSessionId, text);
      const assistantText = extractAssistantText(response);
      if (assistantText) {
        setMessages((current) => [
          ...current,
          {
            id: createMessageId(),
            role: "agent",
            body: assistantText,
            meta: "agent"
          }
        ]);
      }
    } catch (error) {
      setApiError(`Could not send message: ${readErrorMessage(error)}`);
    } finally {
      setIsSending(false);
    }
  };

  return (
    <div className="screen-grid chat-screen">
      <aside className="session-rail" aria-label="Session navigation">
        <div className="brand-row">
          <div>
            <span className="brand-mark">GA</span>
            <span className="brand-name">GeneralAgent</span>
          </div>
          <IconButton label="New session">
            <Plus size={15} aria-hidden="true" />
          </IconButton>
        </div>

        <label className="search-box">
          <Search size={14} aria-hidden="true" />
          <span className="sr-only">Search sessions</span>
          <input placeholder="Search sessions..." />
        </label>

        <div className="rail-section">
          <p className="section-label">Recent sessions</p>
          {recentSessions.map((session) => (
            <button
              className="session-row"
              key={session.title}
              type="button"
            >
              <span>
                <strong>{session.title}</strong>
                <small>
                  <span className="status-dot" aria-hidden="true" />
                  {session.state}
                </small>
              </span>
              <time>{session.time}</time>
            </button>
          ))}
        </div>

        <nav className="rail-nav" aria-label="Primary views">
          <button aria-current="page" className="nav-item active" type="button">
            <Bot size={15} aria-hidden="true" />
            Chat
          </button>
          <button
            className="nav-item"
            type="button"
            onClick={() => onNavigate("sessions")}
          >
            <History size={15} aria-hidden="true" />
            Sessions
          </button>
          <button className="nav-item" type="button">
            <Wrench size={15} aria-hidden="true" />
            Skills
          </button>
          <button className="nav-item" type="button">
            <Settings size={15} aria-hidden="true" />
            Settings
          </button>
        </nav>
      </aside>

      <main className="workbench" aria-label="Conversation workbench">
        <header className="mobile-topbar">
          <div>
            <strong>GeneralAgent</strong>
            <span>Provider adapter MVP</span>
          </div>
          <div className="mobile-actions">
            <IconButton label="Refresh session">
              <RefreshCw size={16} aria-hidden="true" />
            </IconButton>
            <IconButton label="Open sessions" onClick={() => onNavigate("sessions")}>
              <History size={16} aria-hidden="true" />
            </IconButton>
          </div>
        </header>

        <header className="workbench-header">
          <div>
            <h1>Provider adapter MVP</h1>
            <p>chat/completions</p>
          </div>
          <span className="status-pill">
            <span className="status-dot" aria-hidden="true" />
            {isSending ? "Sending" : "Running"}
          </span>
        </header>

        <div className="mobile-chip-row" aria-label="Runtime chips">
          <span>chat/completions</span>
          <span>GPT-4o-mini</span>
          <span>skills: 3</span>
        </div>

        <div className="conversation-log" aria-live="polite">
          <span className="date-chip">Today</span>
          {messages.map((message) => (
            <article
              className={`message-card message-card-${message.role}`}
              key={message.id}
            >
              <div className="message-meta">
                <span>{message.role}</span>
                <time>{message.meta}</time>
              </div>
              <p>{message.body}</p>
            </article>
          ))}
        </div>

        <form
          aria-label="Agent composer"
          className="composer"
          onSubmit={handleSubmit}
        >
          <div className="composer-tools">
            <button className="model-select" type="button">
              GPT-4o
            </button>
            <button aria-pressed="true" className="tool-toggle active" type="button">
              <Terminal size={13} aria-hidden="true" />
              Terminal
            </button>
            <button aria-pressed="false" className="tool-toggle" type="button">
              <Wrench size={13} aria-hidden="true" />
              Web
            </button>
            <button aria-pressed="false" className="tool-toggle" type="button">
              <FileText size={13} aria-hidden="true" />
              Files
            </button>
          </div>
          {apiError ? (
            <p className="composer-error" role="alert">
              {apiError}
            </p>
          ) : null}
          <div className="composer-input-row">
            <IconButton label="Attach context">
              <Paperclip size={16} aria-hidden="true" />
            </IconButton>
            <label className="sr-only" htmlFor="agent-message">
              Message agent
            </label>
            <input
              id="agent-message"
              placeholder="Message agent..."
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
            />
            <IconButton disabled={isSending} label="Send message" type="submit">
              <Send size={17} aria-hidden="true" />
            </IconButton>
          </div>
        </form>
      </main>

      <aside className="inspector-panel" aria-label="Runtime inspector">
        <div className="panel-heading">
          <h2>Inspector</h2>
          <span className="status-pill compact">Active</span>
        </div>
        <section className="inspector-block">
          <p className="section-label">Model configuration</p>
          <dl className="kv-grid">
            <div>
              <dt>Model</dt>
              <dd>GPT-4o-mini</dd>
            </div>
            <div>
              <dt>Context</dt>
              <dd>persistent</dd>
            </div>
          </dl>
        </section>
        <section className="inspector-block metric-grid">
          <div>
            <span>Tokens</span>
            <strong>{tokenUsage}</strong>
          </div>
          <div>
            <span>Latency</span>
            <strong>240ms</strong>
          </div>
        </section>
        <section className="inspector-block">
          <p className="section-label">Enabled skills</p>
          {["bash_execution", "file_system", "web_search"].map((skill) => (
            <div className="skill-row" key={skill}>
              <span className="status-dot" aria-hidden="true" />
              {skill}
              <Play size={12} aria-hidden="true" />
            </div>
          ))}
        </section>
        <section className="inspector-block runtime-log">
          <p className="section-label">Runtime log</p>
          <pre>{`[18:42:01] session initialized
[18:42:01] binding model
[18:42:02] load_skill: file_system
[18:42:03] user prompt received
[18:42:03] execution successful`}</pre>
        </section>
        <section className="inspector-block">
          <p className="section-label">Activity</p>
          <div className="activity-row">
            <Activity size={14} aria-hidden="true" />
            agent-server online
          </div>
        </section>
      </aside>
    </div>
  );
}

function readErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message) {
    return error.message;
  }

  return "unknown error";
}
