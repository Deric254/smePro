// A small, safe markdown-ish renderer for AI chat replies.
//
// The AI providers are now instructed (see ai_assistant.rs's system
// prompt) not to use markdown at all — but a provider can still ignore
// that instruction, and any message already saved to history before
// this existed was written under the old, unrestricted prompt. This is
// defense in depth for both cases: it recognizes just enough markdown
// (bold, headers, bullet/numbered lists, tables, inline code, line
// breaks) to render something clean either way, instead of a customer ever seeing
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
        <code key={key} style={{ background: 'var(--paper-line)', padding: '0.1em 0.35em', borderRadius: 4, fontSize: '0.9em', overflowWrap: 'anywhere' }}>
          {part.slice(1, -1)}
        </code>
      );
    }
    return part;
  });
}

/** Splits one `| a | b |` row into trimmed cells. */
function parseTableRow(line: string): string[] {
  return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((c) => c.trim());
}

/** True for the `|---|:---:|` divider row that follows a table header. */
function isTableDivider(line: string): boolean {
  return /^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?$/.test(line.trim());
}

const cellStyle = { padding: '0.25em 0.3em', borderBottom: '1px solid var(--paper-line)', textAlign: 'left' as const };

/** Numbers and dates never split across lines; any other text may wrap. */
function cellWrap(cell: string): { whiteSpace: 'nowrap' | 'normal' } {
  return { whiteSpace: /^[\d.,:/-]+$/.test(cell) ? 'nowrap' : 'normal' };
}

/** Lets values like `owed_to_business` wrap after an underscore. */
function allowBreaks(nodes: ReactNode[], keyPrefix: string): ReactNode[] {
  const out: ReactNode[] = [];
  nodes.forEach((node, i) => {
    if (typeof node !== 'string') {
      out.push(node);
      return;
    }
    node.split('_').forEach((part, j, parts) => {
      if (j < parts.length - 1) out.push(`${part}_`, <wbr key={`${keyPrefix}-${i}-${j}`} />);
      else out.push(part);
    });
  });
  return out;
}

/** True when the text contains at least one real table (see MarkdownLite). */
export function hasTable(text: string): boolean {
  const lines = text.split('\n');
  return lines.some((line, i) => line.trimStart().startsWith('|') && isTableDivider(lines[i + 1] ?? ''));
}

/** Renders a full AI (or user) message: headers, lists, tables, paragraphs. */
export function MarkdownLite({ text }: { text: string }) {
  const lines = text.split('\n');
  const blocks: ReactNode[] = [];
  let listItems: string[] = [];
  let listOrdered = false;
  let tableRows: string[][] = [];

  function flushTable() {
    if (tableRows.length === 0) return;
    const [header, ...body] = tableRows;
    // The wrapper scrolls sideways so a wide table never stretches the
    // narrow chat bubble.
    blocks.push(
      <div key={`table-${blocks.length}`} style={{ overflowX: 'auto', margin: '0.4em 0' }}>
        <table style={{ borderCollapse: 'collapse', fontSize: '0.75rem' }}>
          <thead>
            <tr>
              {header.map((cell, c) => (
                <th key={c} style={{ ...cellStyle, ...cellWrap(cell), borderBottom: '2px solid var(--paper-line)' }}>{allowBreaks(renderInline(cell, `th-${blocks.length}-${c}`), `thb-${blocks.length}-${c}`)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {body.map((row, r) => (
              <tr key={r}>
                {row.map((cell, c) => (
                  <td key={c} style={{ ...cellStyle, ...cellWrap(cell) }}>{allowBreaks(renderInline(cell, `td-${blocks.length}-${r}-${c}`), `tdb-${blocks.length}-${r}-${c}`)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    );
    tableRows = [];
  }

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

    // A table is a `|` row followed by a `|---|` divider row, then more
    // `|` rows. A lone line that merely starts with `|` stays plain text.
    const startsTable = line.trimStart().startsWith('|') && (tableRows.length > 0 || isTableDivider(lines[i + 1] ?? ''));
    if (startsTable) {
      flushList();
      if (!isTableDivider(line)) tableRows.push(parseTableRow(line));
      return;
    }
    flushTable();

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
  flushTable();

  return <>{blocks}</>;
}
