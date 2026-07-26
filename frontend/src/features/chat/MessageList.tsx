import { useState } from "react";
import { Virtuoso } from "react-virtuoso";
import { Markdown } from "./Markdown";

/** A tool call and, once it resolves, its outcome. */
export type ToolActivity = {
  callId: string;
  name: string;
  args: unknown;
  /** `awaiting` means the run is blocked on the user. */
  status: "running" | "awaiting" | "ok" | "failed" | "denied";
  output?: unknown;
};

export type Turn = {
  id: string;
  role: "system" | "user" | "assistant";
  content: string;
  /** Position in the stored transcript; absent until the turn is persisted. */
  seq?: number;
  pending?: boolean;
  /** Tool traffic that happened while producing this turn. */
  tools?: ToolActivity[];
};

function formatArgs(args: unknown) {
  if (args === null || args === undefined) return "";
  const text = JSON.stringify(args, null, 2);
  return text === "{}" ? "" : text;
}

function ToolCard({
  activity,
  onRespond,
}: {
  activity: ToolActivity;
  onRespond: (callId: string, approved: boolean) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const args = formatArgs(activity.args);

  return (
    <div className={`tool tool--${activity.status}`}>
      <button
        type="button"
        className="tool__head"
        onClick={() => setExpanded((open) => !open)}
      >
        <span className="tool__name">{activity.name}</span>
        <span className="tool__status">
          {activity.status === "running" && "running…"}
          {activity.status === "awaiting" && "needs approval"}
          {activity.status === "ok" && "done"}
          {activity.status === "failed" && "failed"}
          {activity.status === "denied" && "declined"}
        </span>
      </button>

      {activity.status === "awaiting" && (
        <div className="tool__approval">
          <p className="tool__ask">
            Allow <strong>{activity.name}</strong> to run?
          </p>
          {args && <pre className="tool__args">{args}</pre>}
          <div className="tool__actions">
            <button
              type="button"
              className="button button--ghost"
              onClick={() => onRespond(activity.callId, false)}
            >
              Deny
            </button>
            <button
              type="button"
              className="button button--send"
              onClick={() => onRespond(activity.callId, true)}
            >
              Allow
            </button>
          </div>
        </div>
      )}

      {expanded && activity.status !== "awaiting" && (
        <div className="tool__detail">
          {args && (
            <>
              <span className="tool__label">arguments</span>
              <pre className="tool__args">{args}</pre>
            </>
          )}
          {activity.output !== undefined && (
            <>
              <span className="tool__label">result</span>
              <pre className="tool__args">
                {JSON.stringify(activity.output, null, 2)}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function TurnView({
  turn,
  isLastAssistant,
  isBusy,
  onRegenerate,
  onEdit,
  onRespond,
}: {
  turn: Turn;
  isLastAssistant: boolean;
  isBusy: boolean;
  onRegenerate: () => void;
  onEdit: (next: string) => void;
  onRespond: (callId: string, approved: boolean) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(turn.content);

  // Editing and regenerating both rewrite the transcript, so they need a
  // persisted position to truncate from.
  const canRewrite = !isBusy && turn.seq !== undefined;

  const submit = () => {
    const next = draft.trim();
    setEditing(false);
    if (next && next !== turn.content) onEdit(next);
  };

  return (
    <article className={`turn turn--${turn.role}`}>
      <div className="turn__role">
        {turn.role === "user" ? "You" : "Assistant"}
      </div>

      <div className="turn__body">
        {editing ? (
          <div className="turn__edit">
            <textarea
              className="composer__input"
              value={draft}
              autoFocus
              rows={Math.min(12, draft.split("\n").length + 1)}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  submit();
                }
                if (event.key === "Escape") {
                  setDraft(turn.content);
                  setEditing(false);
                }
              }}
            />
            <div className="turn__edit-actions">
              <button
                type="button"
                className="button button--ghost"
                onClick={() => {
                  setDraft(turn.content);
                  setEditing(false);
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                className="button button--send"
                onClick={submit}
              >
                Send
              </button>
            </div>
          </div>
        ) : (
          <>
            {turn.tools && turn.tools.length > 0 && (
              <div className="tool-list">
                {turn.tools.map((activity) => (
                  <ToolCard
                    key={activity.callId}
                    activity={activity}
                    onRespond={onRespond}
                  />
                ))}
              </div>
            )}

            {turn.role === "user" ? (
              <span className="turn__plain">{turn.content}</span>
            ) : (
              <Markdown content={turn.content} live={turn.pending} />
            )}
            {turn.pending && !turn.content && (
              <span className="caret" aria-label="waiting" />
            )}

            <div className="turn__actions">
              {turn.role === "user" && canRewrite && (
                <button
                  type="button"
                  className="turn__action"
                  onClick={() => {
                    setDraft(turn.content);
                    setEditing(true);
                  }}
                >
                  Edit
                </button>
              )}
              {isLastAssistant && canRewrite && (
                <button
                  type="button"
                  className="turn__action"
                  onClick={onRegenerate}
                >
                  Regenerate
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </article>
  );
}

/** Breathing room between the header divider and the first message. */
function MessagesLead() {
  return <div className="messages__lead" />;
}

export function MessageList({
  turns,
  isBusy,
  onRegenerate,
  onEdit,
  onRespond,
}: {
  turns: Turn[];
  isBusy: boolean;
  onRegenerate: () => void;
  onEdit: (turn: Turn, next: string) => void;
  onRespond: (callId: string, approved: boolean) => void;
}) {
  const lastAssistantId = [...turns]
    .reverse()
    .find((turn) => turn.role === "assistant")?.id;

  return (
    <Virtuoso
      className="messages"
      data={turns}
      // Padding on the scroller does not separate the first message from the
      // header: Virtuoso positions items with transforms, so it scrolls under
      // rather than starting below. A Header component is the supported way
      // to reserve that space.
      components={{ Header: MessagesLead }}
      // Stick to the bottom while tokens arrive, but stop fighting the user
      // if they have scrolled up to read something.
      followOutput="auto"
      initialTopMostItemIndex={Math.max(0, turns.length - 1)}
      itemContent={(_index, turn) => (
        <TurnView
          turn={turn}
          isLastAssistant={turn.id === lastAssistantId}
          isBusy={isBusy}
          onRegenerate={onRegenerate}
          onEdit={(next) => onEdit(turn, next)}
          onRespond={onRespond}
        />
      )}
    />
  );
}
