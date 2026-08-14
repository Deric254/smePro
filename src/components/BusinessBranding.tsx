import React, { useState, useEffect } from 'react';
import { getToken, API_BASE as API } from '../api';

export default function BusinessBranding() {
  const [slogan, setSlogan] = useState('');
  const [logoPreview, setLogoPreview] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState('');
  // The flat tax rate applied to every real sale and invoice — see
  // pos.rs / invoice.rs. This used to have no working way to be set
  // anywhere in the app at all: the backend function that updates it
  // existed but had zero HTTP route calling it, so every business was
  // permanently stuck at the schema's 0.0 default.
  const [taxRateText, setTaxRateText] = useState('0');
  const [taxSaving, setTaxSaving] = useState(false);
  const [taxMessage, setTaxMessage] = useState('');

  useEffect(() => {
    fetch(`${API}/business/branding`, { headers: { Authorization: `Bearer ${getToken()}` } })
      .then(r => r.json())
      .then(data => {
        if (data.slogan) { setSlogan(data.slogan); }
        if (data.logo_path) setLogoPreview(`${API}/uploads/${data.logo_path.split('/').pop()}`);
        if (typeof data.tax_rate === 'number') setTaxRateText(String(data.tax_rate));
      });
  }, []);

  async function handleSaveTaxRate(e: React.FormEvent) {
    e.preventDefault();
    setTaxMessage('');
    const rate = parseFloat(taxRateText);
    if (Number.isNaN(rate) || rate < 0 || rate > 100) {
      setTaxMessage('Enter a rate between 0 and 100.');
      return;
    }
    setTaxSaving(true);
    try {
      const res = await fetch(`${API}/business/settings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${getToken()}` },
        body: JSON.stringify({ tax_rate: rate }),
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setTaxMessage('Tax rate updated — applies to sales and invoices from now on.');
    } catch (err: any) {
      setTaxMessage(err.message);
    } finally {
      setTaxSaving(false);
    }
  }

  const handleLogoChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    if (file.size > 2 * 1024 * 1024) {
      setMessage('Logo must be under 2MB');
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      setLogoPreview(result); // data:url for preview
    };
    reader.readAsDataURL(file);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setMessage('');

    const body: any = { slogan: slogan.trim() };
    if (logoPreview && logoPreview.startsWith('data:')) {
      body.logo_base64 = logoPreview.split(',')[1];
    }

    try {
      const res = await fetch(`${API}/business/branding`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${getToken()}` },
        body: JSON.stringify(body),
      });
      const data = await res.json();
      if (data.error) throw new Error(data.error);
      setMessage('Branding updated successfully');
    } catch (err: any) {
      setMessage(err.message);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{ maxWidth: 480, padding: 20 }}>
      <h2 style={{ fontFamily: "Newsreader, Georgia, serif", fontSize: 22, marginBottom: 16 }}>Business Branding</h2>

      {message && (
        <div style={{
          padding: '10px 14px', borderRadius: 8, marginBottom: 16,
          background: message.includes('Error') || message.includes('must') ? '#fdeaea' : '#eafaf1',
          color: message.includes('Error') || message.includes('must') ? '#c0392b' : '#27ae60',
          fontSize: 13,
        }}>{message}</div>
      )}

      <form onSubmit={handleSubmit}>
        <div style={{ marginBottom: 16 }}>
          <label style={{ display:'block', fontSize:13, fontWeight:600, marginBottom:6 }}>Business Logo</label>
          {logoPreview && (
            <img src={logoPreview} alt="Preview" style={{ maxHeight:80, marginBottom:8, objectFit:'contain' }} />
          )}
          <input
            type="file"
            accept="image/png,image/jpeg,image/svg+xml"
            onChange={handleLogoChange}
            style={{ fontSize:13 }}
          />
          <p style={{ fontSize:11, color:'#888', marginTop:4 }}>PNG, JPG, or SVG. Max 2MB.</p>
        </div>

        <div style={{ marginBottom: 16 }}>
          <label style={{ display:'block', fontSize:13, fontWeight:600, marginBottom:6 }}>Slogan</label>
          <input
            type="text"
            value={slogan}
            onChange={e => setSlogan(e.target.value)}
            placeholder="e.g. Quality you can trust"
            maxLength={200}
            style={{
              width:'100%', padding:'10px 12px', borderRadius:8, border:'1px solid #ccc',
              fontSize:14, fontFamily:"'IBM Plex Sans', system-ui, sans-serif",
            }}
          />
          <p style={{ fontSize:11, color:'#888', marginTop:4, textAlign:'right' }}>{slogan.length}/200</p>
        </div>

        <button
          type="submit"
          disabled={loading}
          style={{
            padding:'10px 20px', borderRadius:8, border:'none', background:'#1a1a1a',
            color:'#fff', fontSize:13, fontWeight:600, cursor: loading ? 'not-allowed' : 'pointer',
            opacity: loading ? 0.6 : 1,
          }}
        >
          {loading ? 'Saving…' : 'Save Branding'}
        </button>
      </form>

      <form onSubmit={handleSaveTaxRate} style={{ marginTop: 28, paddingTop: 20, borderTop: '1px solid #eee' }}>
        <label style={{ display:'block', fontSize:13, fontWeight:600, marginBottom:6 }}>Tax rate</label>
        <p style={{ fontSize: 11, color: '#888', marginTop: 0, marginBottom: 8 }}>
          The flat percentage applied to every sale and invoice — e.g. 16 for 16%. Only the owner can change this.
        </p>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <input
            type="text"
            inputMode="decimal"
            value={taxRateText}
            onChange={e => setTaxRateText(e.target.value)}
            style={{ width: 100, padding: '10px 12px', borderRadius: 8, border: '1px solid #ccc', fontSize: 14 }}
          />
          <span style={{ fontSize: 14 }}>%</span>
          <button
            type="submit"
            disabled={taxSaving}
            style={{
              padding: '10px 18px', borderRadius: 8, border: '1px solid #1a1a1a', background: '#fff',
              color: '#1a1a1a', fontSize: 13, fontWeight: 600, cursor: taxSaving ? 'not-allowed' : 'pointer',
            }}
          >
            {taxSaving ? 'Saving…' : 'Save rate'}
          </button>
        </div>
        {taxMessage && (
          <div style={{
            marginTop: 10, fontSize: 12,
            color: taxMessage.includes('Enter') || taxMessage.toLowerCase().includes('error') ? '#c0392b' : '#27ae60',
          }}>{taxMessage}</div>
        )}
      </form>
    </div>
  );
}
