/**
 * Unified Tauri IPC request layer.
 *
 * Dev mode:  -> Vite proxy (/__tauri__) -> tiny_http dev server on :1421
 * Prod mode: -> Tauri's IPC invoke (window.__TAURI__.core.invoke)
 *
 * Vite proxy is required because Electron/Cursor webviews block
 * localhost requests by default (CORB policy).
 */

const isDev = import.meta.env.DEV;

/**
 * Vite development mode is also used by the desktop application.  It must not
 * be confused with running the frontend in a regular browser: the former has
 * Tauri's IPC bridge and can receive live window events, while the latter must
 * use the Dev HTTP proxy.
 */
export function isTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

interface InvokeResult<T> {
  ok: boolean;
  data?: T;
  error?: string;
}

async function parseResult<T>(resp: Response): Promise<T> {
  const result = (await resp.json()) as InvokeResult<T>;
  if (!result.ok) {
    throw new Error(result.error ?? 'Unknown IPC error');
  }
  return result.data as T;
}

/** Dev-mode invoke via Vite proxy (bypasses webview localhost restriction) */
async function invokeViaProxy<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const url = `/__tauri__/__invoke__`;
  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ cmd, args: args ?? null }),
  });
  if (!resp.ok) {
    throw new Error(`IPC proxy error: ${resp.status}`);
  }
  return parseResult<T>(resp);
}

/** Production invoke via Tauri core */
async function invokeViaTauri<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  // Dynamic import to avoid breaking dev server browser startup
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

/**
 * Primary IPC call entry point.
 * Automatically routes to proxy (dev) or Tauri invoke (prod).
 */
export async function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  // Prefer the native bridge whenever this bundle is hosted inside the Tauri
  // WebView, including `tauri dev`. Routing that case through Dev HTTP makes
  // a long-running send_message request block the polling fallback, so live
  // agent events cannot reach the UI until the request finishes.
  if (isTauriRuntime()) {
    return invokeViaTauri<T>(cmd, args);
  }
  if (isDev) {
    return invokeViaProxy<T>(cmd, args);
  }
  return invokeViaTauri<T>(cmd, args);
}
