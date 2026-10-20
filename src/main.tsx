import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import './styles/mobile.css'
import App from './App.tsx'
import { ConfirmHost } from './components/ConfirmDialog'
import LicenceGate from './components/LicenceGate'
import { initApiBase } from './api'

initApiBase().finally(() => createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <LicenceGate>
      <App />
    </LicenceGate>
    <ConfirmHost />
  </StrictMode>,
))
