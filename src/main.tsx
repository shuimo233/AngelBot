import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { isTauriRuntime } from './lib/invoke';

async function bootstrap() {
  if (isTauriRuntime() && new URLSearchParams(location.search).has('desktop-pet')) {
    document.title = 'AngelBot 桌宠';
    const { DesktopPetWindow } = await import('./components/DesktopPet/DesktopPetWindow');
    createRoot(document.getElementById('root')!).render(<StrictMode><DesktopPetWindow /></StrictMode>);
    return;
  }

  if (import.meta.env.VITE_ANGELBOT_DESKTOP_E2E === '1') {
    await import('@wdio/tauri-plugin');
  }

  const { MainWindow } = await import('./MainWindow');
  createRoot(document.getElementById('root')!).render(<StrictMode><MainWindow /></StrictMode>);
}

void bootstrap();
