import type { HighlighterCore } from "shiki/core";

/**
 * Syntax highlighting, imported grammar by grammar and loaded on demand.
 *
 * Two things are deliberate here:
 *
 * 1. Importing `createHighlighter` from "shiki" pulls the full bundle — every
 *    grammar shiki ships plus the Oniguruma wasm engine, ~11 MB of assets for
 *    a handful of languages anyone will use in this app. The core API with
 *    explicit grammar imports ships only what is listed below, and the
 *    JavaScript regex engine removes the wasm dependency entirely.
 *
 * 2. Those grammars are still ~1.4 MB, which has no business sitting in the
 *    startup path of an app whose target is a fast cold start. Everything is
 *    behind a dynamic import, so it loads the first time a code fence is
 *    actually rendered.
 *
 * Anything not listed falls back to unhighlighted text, which is a fine
 * degradation. Add a grammar here when it starts mattering.
 */
const SUPPORTED = new Set([
  "bash",
  "css",
  "diff",
  "go",
  "html",
  "java",
  "javascript",
  "json",
  "markdown",
  "python",
  "rust",
  "sql",
  "toml",
  "tsx",
  "typescript",
  "yaml",
]);

/** Common aliases a model is likely to put after a fence. */
const ALIASES: Record<string, string> = {
  ts: "typescript",
  js: "javascript",
  jsx: "tsx",
  py: "python",
  rs: "rust",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  console: "bash",
  yml: "yaml",
  md: "markdown",
  golang: "go",
};

export const LIGHT_THEME = "github-light";
export const DARK_THEME = "github-dark";

let instance: Promise<HighlighterCore> | null = null;

/**
 * Lazily create the shared highlighter.
 *
 * Creating one is expensive and the grammars are a large download, so this
 * happens once on first use and is reused across every message render for the
 * life of the app.
 */
export function getHighlighter(): Promise<HighlighterCore> {
  instance ??= (async () => {
    const [
      { createHighlighterCore },
      { createJavaScriptRegexEngine },
      githubDark,
      githubLight,
      ...langs
    ] = await Promise.all([
      import("shiki/core"),
      import("shiki/engine/javascript"),
      import("shiki/themes/github-dark.mjs"),
      import("shiki/themes/github-light.mjs"),
      import("shiki/langs/bash.mjs"),
      import("shiki/langs/css.mjs"),
      import("shiki/langs/diff.mjs"),
      import("shiki/langs/go.mjs"),
      import("shiki/langs/html.mjs"),
      import("shiki/langs/java.mjs"),
      import("shiki/langs/javascript.mjs"),
      import("shiki/langs/json.mjs"),
      import("shiki/langs/markdown.mjs"),
      import("shiki/langs/python.mjs"),
      import("shiki/langs/rust.mjs"),
      import("shiki/langs/sql.mjs"),
      import("shiki/langs/toml.mjs"),
      import("shiki/langs/tsx.mjs"),
      import("shiki/langs/typescript.mjs"),
      import("shiki/langs/yaml.mjs"),
    ]);

    return createHighlighterCore({
      themes: [githubDark, githubLight],
      langs,
      engine: createJavaScriptRegexEngine(),
    });
  })();

  return instance;
}

/** Resolve a fence tag to a bundled language, or `null` if unsupported. */
export function resolveLanguage(tag: string | undefined): string | null {
  if (!tag) return null;
  const normalized = tag.toLowerCase().trim();
  if (SUPPORTED.has(normalized)) return normalized;

  const alias = ALIASES[normalized];
  return alias && SUPPORTED.has(alias) ? alias : null;
}
