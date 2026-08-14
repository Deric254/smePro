import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import './styles/mobile.css'
import App from './App.tsx'
import ConnectToHost from './components/ConnectToHost.tsx'
import { setApiBase } from './api'

interface NetworkModeConfig {
  mode: string;
  host_address: string | null;
}

// Resolves this device's network mode (see network_mode.rs) BEFORE
// anything renders, so App never gets a chance to fire its first API
// call against the wrong base URL. Falls back to standalone (api.ts's
// own built-in default, unchanged) on any failure — including running
// in a plain browser with no Tauri IPC bridge at all (e.g. `npm run
// dev` outside the Tauri shell), which is exactly the same as every
// install has always behaved, so this bootstrap can't be the thing
// that breaks that workflow.
async function loadNetworkMode(): Promise<NetworkModeConfig> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    return await invoke<NetworkModeConfig>('get_network_mode');
  } catch {
    return { mode: 'standalone', host_address: null };
  }
}

// One root, created once — re-rendering different content into it
// (the "connect to host" screen, then the real App once paired) is a
// normal `.render()` call on the SAME root, not a second `createRoot`
// on the same DOM node, which React itself warns is invalid.
const root = createRoot(document.getElementById('root')!);

function renderApp() {
  root.render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

loadNetworkMode().then((config) => {
  if (config.mode === 'client') {
    if (config.host_address) {
      setApiBase(`http://${config.host_address}`);
      renderApp();
      return;
    }
    // Client mode selected but never actually paired with a host —
    // show the connect screen instead of an App that has nowhere to
    // send its first request.
    root.render(
      <StrictMode>
        <ConnectToHost
          onConnected={(address) => {
            setApiBase(`http://${address}`);
            renderApp();
          }}
        />
      </StrictMode>,
    );
    return;
  }
  renderApp();
});
