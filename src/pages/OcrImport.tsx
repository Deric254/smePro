import { useState } from 'react';
import { ocrExtractText, ocrParseCandidates, bulkCreateRecords, ApiError } from '../api';
import type { FieldDef } from '../types';

type Step = 'upload' | 'extracting' | 'review' | 'importing' | 'done';

export default function OcrImport({ moduleId, fields, onClose, onImported }: {
  moduleId: string;
  fields: FieldDef[];
  onClose: () => void;
  onImported: () => void;
}) {
  const [step, setStep] = useState<Step>('upload');
  const [rawText, setRawText] = useState('');
  const [candidates, setCandidates] = useState<Record<string, string>[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<{ created: number; errors: { index: number; error: string }[] } | null>(null);

  function fileToBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        // reader.result is "data:image/png;base64,AAAA..." — strip the prefix
        const result = reader.result as string;
        resolve(result.split(',')[1] ?? '');
      };
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }

  async function handleFileSelect(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setError(null);
    setStep('extracting');
    try {
      const base64 = await fileToBase64(file);
      const { raw_text } = await ocrExtractText(base64);
      setRawText(raw_text);
      const { candidates: parsed } = await ocrParseCandidates(moduleId, raw_text);
      // Every value normalized to a string for editing — converted back
      // to the right type on import the same way the manual record form
      // already does.
      setCandidates(parsed.map((c) => {
        const row: Record<string, string> = {};
        for (const f of fields) {
          const v = c[f.name];
          row[f.name] = v === null || v === undefined ? '' : String(v);
        }
        return row;
      }));
      setStep('review');
    } catch (err) {
      const message = err instanceof ApiError ? err.message : 'Could not read this image';
      // "tesseract" not installed is by far the most likely real-world
      // cause — give a specific, actionable message instead of a raw
      // error, since almost nobody will know what that word means.
      if (message.toLowerCase().includes('tesseract')) {
        setError('Photo import needs an extra program (tesseract-ocr) installed on this computer, which isn\u2019t set up yet. Ask whoever installed this app to add it, or enter this record by hand instead.');
      } else {
        setError(message);
      }
      setStep('upload');
    }
  }

  function updateCandidate(index: number, field: string, value: string) {
    setCandidates((prev) => prev.map((row, i) => (i === index ? { ...row, [field]: value } : row)));
  }

  function removeCandidate(index: number) {
    setCandidates((prev) => prev.filter((_, i) => i !== index));
  }

  async function handleImport() {
    setStep('importing');
    setError(null);
    try {
      const records = candidates.map((row) => {
        const rec: Record<string, unknown> = {};
        for (const f of fields) {
          const raw = row[f.name];
          if (raw === '' || raw === undefined) continue; // let required-field validation catch missing ones per-row
          if (f.type === 'integer') rec[f.name] = parseInt(raw, 10);
          else if (f.type === 'real') rec[f.name] = parseFloat(raw);
          else if (f.type === 'boolean') rec[f.name] = raw === 'true';
          else rec[f.name] = raw;
        }
        return rec;
      });
      const res = await bulkCreateRecords(moduleId, records);
      setResult(res);
      setStep('done');
      if (res.created > 0) onImported();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Could not import these records');
      setStep('review');
    }
  }

  return (
    <div style={styles.overlay}>
      <div className="card" style={styles.modal}>
        <div style={styles.header}>
          <h3 style={{ margin: 0 }}>Import from a photo</h3>
          <button className="btn btn-outline" style={{ padding: '0.25em 0.6em', fontSize: '0.78rem' }} onClick={onClose}>Close</button>
        </div>

        {step === 'upload' && (
          <div>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              Photograph a page from a paper ledger or receipt book, and this reads it and
              guesses at the records — you review and fix anything before it actually saves.
            </p>
            {error && <div style={styles.error}>{error}</div>}
            <input type="file" accept="image/*" onChange={handleFileSelect} />
          </div>
        )}

        {step === 'extracting' && (
          <div style={{ padding: '2rem 0', textAlign: 'center', color: 'var(--ink-soft)' }}>Reading the photo…</div>
        )}

        {step === 'review' && (
          <div>
            <p style={{ fontSize: '0.85rem', color: 'var(--ink-soft)' }}>
              Check every row below before importing — this is a best-effort guess, not a
              guaranteed-correct reading. Remove any row that's wrong instead of fixing it, if
              that's faster.
            </p>
            {error && <div style={styles.error}>{error}</div>}
            <details style={{ marginBottom: '0.8rem' }}>
              <summary style={{ fontSize: '0.78rem', color: 'var(--ink-soft)', cursor: 'pointer' }}>Show raw text read from the photo</summary>
              <pre style={styles.rawText}>{rawText}</pre>
            </details>
            <div style={{ overflowX: 'auto' }}>
              <table style={styles.table}>
                <thead>
                  <tr>
                    {fields.map((f) => <th key={f.name} style={styles.th}>{f.name.replace(/_/g, ' ')}</th>)}
                    <th style={styles.th} />
                  </tr>
                </thead>
                <tbody>
                  {candidates.map((row, i) => (
                    <tr key={i}>
                      {fields.map((f) => (
                        <td key={f.name} style={styles.td}>
                          <input
                            value={row[f.name] ?? ''}
                            onChange={(e) => updateCandidate(i, f.name, e.target.value)}
                            style={styles.cellInput}
                          />
                        </td>
                      ))}
                      <td style={styles.td}>
                        <button className="btn btn-outline" style={{ padding: '0.2em 0.5em', fontSize: '0.74rem' }} onClick={() => removeCandidate(i)}>Remove</button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            {candidates.length === 0 && <div style={{ color: 'var(--ink-soft)', fontSize: '0.85rem', marginTop: '0.6rem' }}>Nothing left to import.</div>}
            <button className="btn btn-stamp" style={{ marginTop: '1rem' }} onClick={handleImport} disabled={candidates.length === 0}>
              Import {candidates.length} record{candidates.length === 1 ? '' : 's'}
            </button>
          </div>
        )}

        {step === 'importing' && (
          <div style={{ padding: '2rem 0', textAlign: 'center', color: 'var(--ink-soft)' }}>Importing…</div>
        )}

        {step === 'done' && result && (
          <div>
            <div style={{ color: result.created > 0 ? 'var(--ok)' : 'var(--stamp)', fontWeight: 600, marginBottom: '0.6rem' }}>
              {result.created} record{result.created === 1 ? '' : 's'} imported.
            </div>
            {result.errors.length > 0 && (
              <div style={{ fontSize: '0.82rem', color: 'var(--ink-soft)' }}>
                {result.errors.length} row{result.errors.length === 1 ? '' : 's'} couldn't be imported:
                <ul style={{ margin: '0.4rem 0 0', paddingLeft: '1.2rem' }}>
                  {result.errors.map((e, i) => <li key={i}>Row {e.index + 1}: {e.error}</li>)}
                </ul>
              </div>
            )}
            <button className="btn btn-stamp" style={{ marginTop: '1rem' }} onClick={onClose}>Done</button>
          </div>
        )}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  overlay: {
    position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.4)',
    display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 50, padding: '1.5rem',
  },
  modal: { width: '100%', maxWidth: 720, maxHeight: '85vh', overflowY: 'auto' },
  header: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '0.9rem' },
  error: { background: 'var(--stamp-wash)', color: 'var(--stamp)', padding: '0.6em 0.8em', borderRadius: 3, fontSize: '0.85rem', marginBottom: '0.8rem', lineHeight: 1.4 },
  rawText: { fontSize: '0.75rem', background: 'var(--paper)', padding: '0.6rem', borderRadius: 3, whiteSpace: 'pre-wrap', maxHeight: 150, overflowY: 'auto' },
  table: { width: '100%', borderCollapse: 'collapse', fontSize: '0.82rem' },
  th: { textAlign: 'left', padding: '0.4rem 0.5rem', borderBottom: '1px solid var(--paper-line)', fontSize: '0.7rem', textTransform: 'uppercase', color: 'var(--ink-soft)' },
  td: { padding: '0.3rem 0.5rem', borderBottom: '1px solid var(--paper-line)' },
  cellInput: { width: '100%', fontSize: '0.82rem', padding: '0.3em 0.4em' },
};
