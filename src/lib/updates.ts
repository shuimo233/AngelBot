import { check, type DownloadEvent, type Update } from '@tauri-apps/plugin-updater';

export interface AvailableAppUpdate {
  version: string;
  body?: string;
  install(onEvent?: (event: DownloadEvent) => void): Promise<void>;
  close(): Promise<void>;
}

function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function adaptUpdate(update: Update): AvailableAppUpdate {
  return {
    version: update.version,
    body: update.body,
    install: (onEvent) => update.downloadAndInstall(onEvent, { restartAfterInstall: true }),
    close: () => update.close(),
  };
}

/**
 * Quietly checks for a signed production update.
 *
 * Development, test, and unsigned local builds intentionally have no updater
 * plugin. Network or plugin availability must never make the main workspace
 * unusable, so startup checks are best-effort and installation errors are
 * handled separately by the UI after the user explicitly starts an update.
 */
export async function checkForAppUpdate(): Promise<AvailableAppUpdate | null> {
  if (!isTauriRuntime()) return null;
  try {
    const update = await check({ timeout: 10_000 });
    return update ? adaptUpdate(update) : null;
  } catch {
    return null;
  }
}
