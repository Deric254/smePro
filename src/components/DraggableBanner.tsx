import { useRef, useState } from 'react';

// Shared draggable/dismissible notification banner (used by
// UpdateChecker.tsx and AndroidUpdateChecker.tsx). Drag uses Pointer
// Events for unified mouse/touch support, applied as a CSS transform
// on top of the caller's own fixed position. Drag position and
// dismissal don't persist across remounts — session-only.
export default function DraggableBanner({
  className,
  style,
  onDismiss,
  children,
}: {
  className?: string;
  style?: React.CSSProperties;
  onDismiss: () => void;
  children: React.ReactNode;
}) {
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const dragRef = useRef<{ startX: number; startY: number; baseX: number; baseY: number; pointerId: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>) {
    // Ignore drags started on an interactive control inside the
    // banner (the dismiss button, the Install/Update button) — only
    // the banner's own body/background should initiate a drag, or
    // clicking those buttons would also nudge the banner a pixel or
    // two on every tap.
    const target = e.target as HTMLElement;
    if (target.closest('button')) return;

    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    dragRef.current = { startX: e.clientX, startY: e.clientY, baseX: offset.x, baseY: offset.y, pointerId: e.pointerId };
    setDragging(true);
  }

  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (!dragRef.current || dragRef.current.pointerId !== e.pointerId) return;
    const dx = e.clientX - dragRef.current.startX;
    const dy = e.clientY - dragRef.current.startY;
    setOffset({ x: dragRef.current.baseX + dx, y: dragRef.current.baseY + dy });
  }

  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>) {
    if (dragRef.current?.pointerId === e.pointerId) {
      dragRef.current = null;
      setDragging(false);
    }
  }

  return (
    <div
      className={className}
      style={{
        ...style,
        transform: `translate(${offset.x}px, ${offset.y}px)`,
        cursor: dragging ? 'grabbing' : 'grab',
        // No CSS transition on transform while actively dragging —
        // it would lag a frame behind the pointer. Fine (and nicer)
        // once released, so a stray small drag doesn't look jerky.
        transition: dragging ? 'none' : 'transform 0.15s ease-out',
        touchAction: 'none', // otherwise a touch-drag also scrolls the page underneath it
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={handlePointerUp}
    >
      <button
        type="button"
        aria-label="Dismiss"
        onClick={onDismiss}
        style={{
          position: 'absolute', top: -8, right: -8, width: 22, height: 22, borderRadius: '50%',
          border: '1px solid var(--paper-line)', background: 'var(--paper)', color: 'var(--ink-soft)',
          fontSize: '0.75rem', lineHeight: 1, cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center',
          padding: 0, boxShadow: '0 1px 3px rgba(0,0,0,0.15)',
        }}
      >
        ✕
      </button>
      {children}
    </div>
  );
}
