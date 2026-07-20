import { useState } from "react";
import { Virtuoso } from "react-virtuoso";
import { Markdown } from "./Markdown";

export type Turn = {
  id: string;
  role: "system" | "user" | "assistant";
  content: string;
  /** Position in the stored transcript; absent until the turn is persisted. */
  seq?: number;
  pending?: boolean;
};

function TurnView({
  turn,
  isLastAssistant,
  isBusy,
  onRegenerate,
  onEdit,
}: {
  turn: Turn;
  isLastAssistant: boolean;
  isBusy: boolean;
  onRegenerate: () => void;
  onEdit: (next: string) => void;
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

export function MessageList({
  turns,
  isBusy,
  onRegenerate,
  onEdit,
}: {
  turns: Turn[];
  isBusy: boolean;
  onRegenerate: () => void;
  onEdit: (turn: Turn, next: string) => void;
}) {
  const lastAssistantId = [...turns]
    .reverse()
    .find((turn) => turn.role === "assistant")?.id;

  return (
    <Virtuoso
      className="messages"
      data={turns}
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
        />
      )}
    />
  );
}
