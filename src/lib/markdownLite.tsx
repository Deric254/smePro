// A small, safe markdown-ish renderer for AI chat replies.
//
// The AI providers are now instructed (see ai_assistant.rs's system
// prompt) not to use markdown at all — but a provider can still ignore
// that instruction, and any message already saved to history before
// this existed was written under the old, unrestricted prompt. This is
// defense in depth for both cases: it recognizes just enough markdown
// (bold, headers, bullet/numbered lists, inline code, line breaks) to
// render something clean either way, instead of a customer ever seeing
// a literal "**Total: $450**" in a chat bubble again.
//
// Deliberately NOT using a markdown library or dangerouslySetInnerHTML
// — this returns real React nodes built from plain string parsing, so
// there's no HTML-injection surface from AI-provided text, ever.

import type { ReactNode } from 'react';

/** Renders one line's inline formatting: **bold** and `code`. */
function renderInline(text: string, keyPrefix: string): ReactNode[] {
  // Splits on **bold** and `code` spans, keeping the delimiters so the
  // matched groups survive the split and can be re-wrapped below.
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).filter((p) => p !== '');
  return parts.map((part, i) => {
    const key = `${keyPrefix}-${i}`;
    if (part.startsWith('**') && part.endsWith('**') && part.length > 4) {
      return <strong key={key}>{part.slice(2, -2)}</strong>;
    }
    if (part.startsWith('`') && part.endsWith('`') && part.length > 2) {
      return (
        <code key={key} style={{ background: 'var(--paper-line)', padding: '0.1em 0.35em', borderRadius: 4, fontSize: '0.9em' }}>
          {part.slice(1, -1)}
        </code>
      );
    }
    return part;
  });
}

/** Renders a full AI (or user) message: headers, lists, paragraphs. */
export function MarkdownLite({ text }: { text: string }) {
  const lines = text.split('\n');
  const blocks: ReactNode[] = [];
  let listItems: string[] = [];
  let listOrdered = false;

  function flushList() {
    if (listItems.length === 0) return;
    const Tag = listOrdered ? 'ol' : 'ul';
    blocks.push(
      <Tag key={`list-${blocks.length}`} style={{ margin: '0.3em 0', paddingLeft: '1.3em' }}>
        {listItems.map((item, i) => (
          <li key={i}>{renderInline(item, `li-${blocks.length}-${i}`)}</li>
        ))}
      </Tag>
    );
    listItems = [];
  }

  lines.forEach((rawLine, i) => {
    const line = rawLine.trimEnd();
    const bulletMatch = line.match(/^\s*[-*]\s+(.*)$/);
    const numberedMatch = line.match(/^\s*\d+[.)]\s+(.*)$/);
    const headerMatch = line.match(/^#{1,6}\s+(.*)$/);

    if (bulletMatch) {
      if (listOrdered) flushList();
      listOrdered = false;
      listItems.push(bulletMatch[1]);
      return;
    }
    if (numberedMatch) {
      if (!listOrdered) flushList();
      listOrdered = true;
      listItems.push(numberedMatch[1]);
      return;
    }
    flushList();

    if (headerMatch) {
      blocks.push(
        <div key={`h-${i}`} style={{ fontWeight: 700, marginTop: blocks.length ? '0.5em' : 0 }}>
          {renderInline(headerMatch[1], `h-${i}`)}
        </div>
      );
      return;
    }
    if (line.trim() === '') {
      // Blank line — a paragraph break, not its own empty element.
      return;
    }
    blocks.push(<div key={`p-${i}`}>{renderInline(line, `p-${i}`)}</div>);
  });
  flushList();

  return <>{blocks}</>;
}
