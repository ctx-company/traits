import type { ResourceHandle } from "./handles.js";
import { headingBlock } from "./normalize.js";
import { resource } from "./procedure.js";

export interface StepsFields {
  readonly id: string;
  readonly title?: string;
  readonly items: readonly string[];
}

type TableRow<Headers extends readonly string[]> = { readonly [Index in keyof Headers]: string; };

export interface SnippetFields {
  readonly id: string;
  readonly lang: string;
  readonly code: string;
}

export interface CalloutFields {
  readonly id: string;
  readonly kind: "note" | "warning" | "tip";
  readonly body: string;
}

/**
 * Creates an ordered inline-content resource, using a level-two title
 * heading — a numbered how-to list a prompt can reference by resource
 * handle instead of a hand-formatted Markdown string embedded in the trait
 * source.
 * @example `steps({ id: "validation-steps", items: ["Run the build", "Run the tests"] })`
 * @see {@link resource}
 */
export function steps(fields: StepsFields): ResourceHandle {
  return resource.inline(
    fields.id,
    headingBlock(fields.title ?? "Steps", fields.items.map((item, index) => `${index + 1}. ${item}`).join("\n")),
  );
}

/**
 * Creates an inline-content Markdown table. Pipes become `\\|` and CRLF, CR, and LF cell breaks become `<br>`.
 * Literal tuple rows must have exactly one cell per header.
 *
 * Hand-writing a Markdown table means manually escaping any `|` or newline
 * that shows up in a cell — easy to miss and prone to breaking the model's
 * rendering. `table(...)` escapes every cell, and its generic `Headers`/
 * `Rows` types check at `tsc` that each row has exactly one cell per header,
 * instead of silently misaligning columns.
 * @example `table({ id: "gates", headers: ["Gate", "Command"], rows: [["build", "pnpm build"]] })`
 * @see {@link resource}
 */
export function table<
  const Headers extends readonly string[],
  const Rows extends readonly TableRow<NoInfer<Headers>>[],
>(
  fields: { readonly id: string; readonly headers: Headers; readonly rows: Rows; },
): ResourceHandle {
  const renderRow = (cells: readonly string[]): string => `| ${cells.map(escapeTableCell).join(" | ")} |`;
  const content = [
    renderRow(fields.headers),
    `| ${fields.headers.map(() => "---").join(" | ")} |`,
    ...fields.rows.map(renderRow),
  ].join("\n") + "\n";
  return resource.inline(fields.id, content);
}

/**
 * Creates an inline-content fenced code block. The closing fence always ends
 * with one LF, and the fence itself widens automatically past any run of
 * backticks already inside `code` — the fence-collision bug a hand-written
 * ` ```lang ` block risks when the snippet contains its own backticks.
 * @example `snippet({ id: "example-diff", lang: "diff", code: "+ added line" })`
 * @see {@link resource}
 */
export function snippet(fields: SnippetFields): ResourceHandle {
  const longestFence = Math.max(0, ...Array.from(fields.code.matchAll(/`+/g), (match) => match[0].length));
  const fence = "`".repeat(Math.max(3, longestFence + 1));
  const separator = fields.code.endsWith("\n") ? "" : "\n";
  return resource.inline(fields.id, `${fence}${fields.lang}\n${fields.code}${separator}${fence}\n`);
}

/**
 * Creates an inline-content GitHub-style callout (`> [!NOTE]`/`[!WARNING]`/
 * `[!TIP]`). Every body line, including blank lines, is quoted, which a
 * hand-written multi-line callout is easy to get wrong on (an unquoted
 * blank line breaks the blockquote in most Markdown renderers).
 * @example `callout({ id: "scope-note", kind: "warning", body: "Do not touch generated.ts." })`
 * @see {@link resource}
 */
export function callout(fields: CalloutFields): ResourceHandle {
  const marker = { note: "NOTE", warning: "WARNING", tip: "TIP" }[fields.kind];
  const body = fields.body.split(/\r\n|\r|\n/).map((line) => `> ${line}`).join("\n");
  return resource.inline(fields.id, `> [!${marker}]\n${body}\n`);
}

function escapeTableCell(value: string): string {
  return value.replace(/\|/g, "\\|").replace(/\r\n|\r|\n/g, "<br>");
}
