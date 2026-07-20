import type * as React from "react";

import { cn } from "../lib/utils";

/** Props for the dependency-free markdown renderer used by thread messages. */
export interface MarkdownProps {
  /** Markdown source text. */
  readonly text: string;
  /** Additional class names for the wrapper. */
  readonly className?: string;
}

/** Renders common markdown blocks without adding a heavy parser dependency to the UI package. */
export function Markdown({ className, text }: MarkdownProps): React.JSX.Element {
  return <div className={cn("space-y-2 text-sm leading-6", className)}>{renderBlocks(text)}</div>;
}

function renderBlocks(text: string): React.ReactNode[] {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const blocks: React.ReactNode[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index] ?? "";
    if (line.trim().length === 0) {
      index += 1;
      continue;
    }

    if (line.trimStart().startsWith("```")) {
      const codeLines: string[] = [];
      index += 1;
      while (index < lines.length && !(lines[index] ?? "").trimStart().startsWith("```")) {
        codeLines.push(lines[index] ?? "");
        index += 1;
      }
      index += 1;
      blocks.push(
        <pre
          className="overflow-x-auto rounded-md border border-border bg-muted p-3 text-xs leading-5 text-foreground"
          key={`code-${blocks.length}`}
        >
          <code>{codeLines.join("\n")}</code>
        </pre>
      );
      continue;
    }

    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading !== null) {
      const level = heading[1]?.length ?? 1;
      const content = heading[2] ?? "";
      const className = cn(
        "font-semibold tracking-tight",
        level === 1 && "text-lg",
        level === 2 && "text-base",
        level === 3 && "text-sm"
      );
      if (level === 1) {
        blocks.push(
          <h1 className={className} key={`heading-${blocks.length}`}>
            {renderInline(content)}
          </h1>
        );
      } else if (level === 2) {
        blocks.push(
          <h2 className={className} key={`heading-${blocks.length}`}>
            {renderInline(content)}
          </h2>
        );
      } else {
        blocks.push(
          <h3 className={className} key={`heading-${blocks.length}`}>
            {renderInline(content)}
          </h3>
        );
      }
      index += 1;
      continue;
    }

    if (/^\s*[-*]\s+/.test(line)) {
      const items: string[] = [];
      while (index < lines.length && /^\s*[-*]\s+/.test(lines[index] ?? "")) {
        items.push((lines[index] ?? "").replace(/^\s*[-*]\s+/, ""));
        index += 1;
      }
      blocks.push(
        <ul className="list-disc space-y-1 pl-5" key={`list-${blocks.length}`}>
          {items.map((item, itemIndex) => (
            <li key={`${itemIndex}-${item}`}>{renderInline(item)}</li>
          ))}
        </ul>
      );
      continue;
    }

    const paragraphLines = [line];
    index += 1;
    while (
      index < lines.length &&
      (lines[index] ?? "").trim().length > 0 &&
      !(lines[index] ?? "").trimStart().startsWith("```") &&
      !/^(#{1,3})\s+/.test(lines[index] ?? "") &&
      !/^\s*[-*]\s+/.test(lines[index] ?? "")
    ) {
      paragraphLines.push(lines[index] ?? "");
      index += 1;
    }
    blocks.push(
      <p className="whitespace-pre-wrap" key={`paragraph-${blocks.length}`}>
        {renderInline(paragraphLines.join("\n"))}
      </p>
    );
  }

  if (blocks.length === 0) {
    return [
      <p className="text-muted-foreground" key="empty">
        Empty message
      </p>
    ];
  }

  return blocks;
}

function renderInline(text: string): React.ReactNode[] {
  const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g);
  return parts.map((part, index) => {
    if (part.startsWith("`") && part.endsWith("`")) {
      return (
        <code className="rounded bg-muted px-1 py-0.5 text-[0.85em]" key={`${index}-${part}`}>
          {part.slice(1, -1)}
        </code>
      );
    }
    if (part.startsWith("**") && part.endsWith("**")) {
      return <strong key={`${index}-${part}`}>{part.slice(2, -2)}</strong>;
    }
    return part;
  });
}
