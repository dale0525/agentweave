import {
  ArrowLeft,
  Bot,
  ChevronRight,
  History,
  Plus,
  Search,
  Settings,
  SlidersHorizontal,
  Wrench
} from "lucide-react";

type AppView = "chat" | "sessions";

type SessionsProps = {
  onNavigate?: (view: AppView) => void;
};

const sessions = [
  {
    title: "Provider adapter MVP",
    branch: "gpt-4o-mini",
    status: "Running",
    time: "10:42 AM",
    detail: "Refactoring the main connection loop and status inspector."
  },
  {
    title: "Schema validation fix",
    branch: "grep-codebase",
    status: "Ready",
    time: "Yesterday",
    detail: "Normalize tool output payloads before persistence."
  },
  {
    title: "API test",
    branch: "GPT-4",
    status: "Ready",
    time: "Oct 24",
    detail: "OpenAI-compatible provider smoke test."
  },
  {
    title: "Local model draft",
    branch: "llama-3",
    status: "Error",
    time: "Oct 22",
    detail: "Connection timeout on staging cluster."
  }
];

export function Sessions({ onNavigate = () => undefined }: SessionsProps): JSX.Element {
  return (
    <div className="screen-grid sessions-screen">
      <aside className="session-rail command-rail" aria-label="Application navigation">
        <div className="brand-row">
          <div>
            <span className="brand-mark">GA</span>
            <span className="brand-name">GeneralAgent</span>
          </div>
        </div>
        <button className="primary-action" type="button">
          <Plus size={15} aria-hidden="true" />
          New session
        </button>
        <nav className="rail-nav" aria-label="Primary views">
          <button className="nav-item" type="button" onClick={() => onNavigate("chat")}>
            <Bot size={15} aria-hidden="true" />
            Chat
          </button>
          <button aria-current="page" className="nav-item active" type="button">
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
        <div className="rail-footer">
          <span className="status-dot" aria-hidden="true" />
          agent-server online
        </div>
      </aside>

      <main className="session-manager" aria-label="Sessions manager">
        <header className="manager-header">
          <div>
            <button
              aria-label="Back to chat"
              className="mobile-back-chat"
              onClick={() => onNavigate("chat")}
              title="Back to chat"
              type="button"
            >
              <ArrowLeft size={17} aria-hidden="true" />
            </button>
            <h1>Sessions</h1>
            <span className="status-pill compact">Online</span>
          </div>
          <label className="search-box manager-search">
            <Search size={14} aria-hidden="true" />
            <span className="sr-only">Search sessions</span>
            <input placeholder="Search sessions..." />
          </label>
          <button className="mobile-new-session" type="button" aria-label="New session" title="New session">
            <Plus size={17} aria-hidden="true" />
          </button>
        </header>

        <div className="filter-row" aria-label="Session filters">
          {["All", "Responses", "Chat", "Completion"].map((filter) => (
            <button
              aria-pressed={filter === "All"}
              className={filter === "All" ? "active" : ""}
              key={filter}
              type="button"
            >
              {filter}
            </button>
          ))}
        </div>

        <div className="session-table" role="table" aria-label="Stored sessions">
          <div className="session-table-head" role="row">
            <span role="columnheader">Title / Context</span>
            <span role="columnheader">Model</span>
            <span role="columnheader">Updated</span>
          </div>
          {sessions.map((session) => (
            <article className="session-table-row" key={session.title} role="row">
              <div role="cell">
                <strong>{session.title}</strong>
                <p>{session.detail}</p>
              </div>
              <span className="model-badge" role="cell">
                {session.branch}
              </span>
              <time role="cell">{session.time}</time>
            </article>
          ))}
        </div>

        <div className="session-card-list" aria-label="Stored sessions">
          {sessions.map((session) => (
            <article className="mobile-session-card" key={session.title}>
              <header>
                <strong>{session.title}</strong>
                <time>{session.time}</time>
              </header>
              <p>{session.detail}</p>
              <footer>
                <span className={`session-state ${session.status.toLowerCase()}`}>
                  <span className="status-dot" aria-hidden="true" />
                  {session.status}
                </span>
                <span className="model-badge">{session.branch}</span>
                <button type="button">
                  Continue
                  <ChevronRight size={14} aria-hidden="true" />
                </button>
              </footer>
            </article>
          ))}
        </div>
      </main>

      <aside className="inspector-panel sessions-inspector" aria-label="Session inspector">
        <div className="panel-heading">
          <h2>Inspector</h2>
          <SlidersHorizontal size={15} aria-hidden="true" />
        </div>
        <section className="inspector-block">
          <p className="section-label">Provider adapter MVP</p>
          <div className="activity-row">
            <span className="status-dot" aria-hidden="true" />
            GPT-4o-mini
          </div>
        </section>
        <section className="inspector-block metric-grid">
          <div>
            <span>Req</span>
            <strong>0.7</strong>
          </div>
          <div>
            <span>Tokens</span>
            <strong>4096</strong>
          </div>
        </section>
        <section className="inspector-block runtime-log">
          <p className="section-label">Active skill</p>
          <pre>{`fetch_github_pr
  repository: general-agent
  pull: 1042

grep_codebase
  query: runtime.strategy`}</pre>
        </section>
        <button className="restore-button" type="button">
          Restore session view
        </button>
      </aside>
    </div>
  );
}
