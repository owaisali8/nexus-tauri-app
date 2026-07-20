import { type ReactNode, memo, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  DARK_THEME,
  LIGHT_THEME,
  getHighlighter,
  resolveLanguage,
} from "./highlighter";

function CodeBlock({
  code,
  language,
  live,
}: {
  code: string;
  language: string | undefined;
  live: boolean;
}) {
  // Keyed by the code it was produced from, so a result that arrives after
  // the content moved on is discarded rather than rendered as stale output.
  const [highlighted, setHighlighted] = useState<{
    code: string;
    html: string;
  } | null>(null);
  const [copied, setCopied] = useState(false);
  const resolved = resolveLanguage(language);

  useEffect(() => {
    // While tokens are still arriving the fence changes on every frame, and
    // re-highlighting each revision burns CPU on output that is about to be
    // replaced. Render plain until the message settles.
    if (live || !resolved) return;

    let cancelled = false;
    void (async () => {
      try {
        const instance = await getHighlighter();
        const rendered = instance.codeToHtml(code, {
          lang: resolved,
          themes: { dark: DARK_THEME, light: LIGHT_THEME },
          defaultColor: "dark",
        });
        if (!cancelled) setHighlighted({ code, html: rendered });
      } catch {
        // An unsupported grammar should degrade to plain text, not break the
        // message.
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [code, resolved, live]);

  const html = !live && highlighted?.code === code ? highlighted.html : null;

  const copy = () => {
    void navigator.clipboard.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };

  return (
    <div className="code">
      <div className="code__bar">
        <span className="code__lang">{resolved ?? language ?? "text"}</span>
        <button type="button" className="code__copy" onClick={copy}>
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      {html ? (
        // Safe: shiki generates this from the code string and escapes its
        // content. No model-authored HTML reaches the DOM here.
        <div
          className="code__body"
          dangerouslySetInnerHTML={{ __html: html }}
        />
      ) : (
        <pre className="code__body code__body--plain">
          <code>{code}</code>
        </pre>
      )}
    </div>
  );
}

/**
 * Flatten a node to its text.
 *
 * `children` is typed as ReactNode, so `String(children)` would stringify an
 * element as "[object Object]". Fences are plain text in practice, but the
 * type permits otherwise.
 */
function nodeText(node: ReactNode): string {
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  return "";
}

/**
 * Render assistant/user text as Markdown.
 *
 * Raw HTML is not enabled, so anything HTML-shaped in model output is shown
 * as text rather than executed.
 */
export const Markdown = memo(function Markdown({
  content,
  live = false,
}: {
  content: string;
  live?: boolean;
}) {
  return (
    <div className="markdown">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // CodeBlock emits its own <pre>; without this the fence would end
          // up nested inside react-markdown's.
          pre: ({ children }) => <>{children}</>,
          code({ className, children, ...rest }) {
            const text = nodeText(children).replace(/\n$/, "");
            const match = /language-([\w-]+)/.exec(className ?? "");

            if (!match && !text.includes("\n")) {
              return (
                <code className="markdown__inline-code" {...rest}>
                  {children}
                </code>
              );
            }

            return (
              <CodeBlock code={text} language={match?.[1]} live={live} />
            );
          },
          a({ href, children }) {
            return (
              <a
                href={href}
                onClick={(event) => {
                  // Following a link in-place would navigate the webview away
                  // from the app itself. Hand it to the OS browser instead.
                  event.preventDefault();
                  if (href) void openUrl(href);
                }}
              >
                {children}
              </a>
            );
          },
        }}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
});
