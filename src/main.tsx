import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './ui/tokens.css';
import './styles/global.css';
import App from './App';
import './styles/product.css';
import './ui/system.css';

async function bootstrap() {
  if (import.meta.env.VITE_ANGELBOT_DESKTOP_E2E === '1') {
    await import('@wdio/tauri-plugin');
  }

  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void bootstrap();
