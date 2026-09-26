
import type { ReactNode } from 'react';

function renderInline(text: string, keyPrefix: string): ReactNode[] {
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
      return;
    }
    blocks.push(<div key={`p-${i}`}>{renderInline(line, `p-${i}`)}</div>);
  });
  flushList();

  return <>{blocks}</>;
}
