import { useEffect, useId, useRef, useState, type ReactNode } from 'react';

/* In-app replacement for window.confirm().
 *
 * window.confirm() renders the browser's own "tauri.localhost says"
 * dialog: unstyled, unbranded, ignores the user's theme, and on Android
 * WebViews it can be suppressed entirely. This is a promise-based
 * drop-in:
 *
 *     if (!(await confirmDialog({ title: '…', tone: 'danger' }))) return;
 *
 * Mount <ConfirmHost /> exactly once (main.tsx).
 *
 * Safety properties, because this guards destructive actions:
 *  - Fail closed: if the host isn't mounted, or is unmounted while a
 *    question is open, the promise resolves FALSE. A destructive action
 *    never proceeds because the dialog failed to appear.
 *  - Danger dialogs focus "Cancel" first, so a stray Enter/Space
 *    keypress can't confirm a destructive action.
 *  - Escape and a click on the backdrop both mean "cancel".
 *  - Concurrent calls are queued, never dropped or stacked.
 */

export interface ConfirmOptions {
  title: string;
  /** Main sentence(s). */
  message?: ReactNode;
  /** Optional bullet list shown below the message. */
  items?: string[];
  /** Small print under the list, e.g. what the action does / that it can't be undone. */
  note?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** 'danger' = destructive styling + Cancel focused by default. */
  tone?: 'default' | 'danger';
}

interface Pending extends ConfirmOptions {
  id: number;
  resolve: (ok: boolean) => void;
}

let enqueue: ((p: Pending) => void) | null = null;
let nextId = 1;

export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    if (!enqueue) {
      console.error('confirmDialog() called but <ConfirmHost /> is not mounted; refusing by default.');
      resolve(false);
      return;
    }
    enqueue({ ...options, id: nextId++, resolve });
  });
}

export function ConfirmHost() {
  const [queue, setQueue] = useState<Pending[]>([]);
  const queueRef = useRef<Pending[]>([]);
  queueRef.current = queue;

  useEffect(() => {
    enqueue = (p) => setQueue((q) => [...q, p]);
    return () => {
      enqueue = null;
      // Fail closed: anything still waiting is answered "no".
      queueRef.current.forEach((p) => p.resolve(false));
    };
  }, []);

  const current = queue[0];
  const settle = (ok: boolean) => {
    if (!current) return;
    current.resolve(ok);
    setQueue((q) => q.slice(1));
  };

  return current ? <ConfirmModal key={current.id} pending={current} onSettle={settle} /> : null;
}

function ConfirmModal({ pending, onSettle }: { pending: Pending; onSettle: (ok: boolean) => void }) {
  const {
    title, message, items, note,
    confirmLabel = 'OK', cancelLabel = 'Cancel', tone = 'default',
  } = pending;
  const danger = tone === 'danger';
  const uid = useId();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    (danger ? cancelRef : confirmRef).current?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onSettle(false);
      } else if (e.key === 'Tab') {
        // Keep focus inside the dialog.
        const a = cancelRef.current, b = confirmRef.current;
        if (!a || !b) return;
        const active = document.activeElement;
        if (e.shiftKey && active === a) { e.preventDefault(); b.focus(); }
        else if (!e.shiftKey && active === b) { e.preventDefault(); a.focus(); }
        else if (active !== a && active !== b) { e.preventDefault(); a.focus(); }
      }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      previouslyFocused?.focus?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div
      className="confirm-overlay"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onSettle(false); }}
    >
      <div
        className={`confirm-dialog${danger ? ' confirm-dialog--danger' : ''}`}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={`${uid}-title`}
        aria-describedby={`${uid}-body`}
      >
        <h2 id={`${uid}-title`} className="confirm-title">{title}</h2>
        <div id={`${uid}-body`} className="confirm-body">
          {message && <p className="confirm-message">{message}</p>}
          {items && items.length > 0 && (
            <ul className="confirm-items">
              {items.map((line, i) => <li key={i}>{line}</li>)}
            </ul>
          )}
          {note && <p className="confirm-note">{note}</p>}
        </div>
        <div className="confirm-actions">
          <button ref={cancelRef} type="button" className="btn btn-outline" onClick={() => onSettle(false)}>
            {cancelLabel}
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={`btn${danger ? ' btn-stamp' : ''}`}
            onClick={() => onSettle(true)}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
