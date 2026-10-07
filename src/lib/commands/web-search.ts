import { invoke } from '../invoke';

export const WEB_SEARCH_PROVIDERS = ['tavily', 'brave', 'exa', 'searxng'] as const;

export type WebSearchProvider = (typeof WEB_SEARCH_PROVIDERS)[number];

/** Metadata returned by the desktop backend. API key material is never returned. */
export interface WebSearchConfig {
  provider: WebSearchProvider;
  endpoint: string;
  apiKeyConfigured: boolean;
  enabled: boolean;
}

export interface ConfigureWebSearchRequest {
  provider: WebSearchProvider;
  apiKey: string;
  /** Required for a self-hosted SearXNG instance; ignored for hosted providers. */
  endpoint?: string;
}

export function getWebSearchConfig(): Promise<WebSearchConfig | null> {
  return invoke<WebSearchConfig | null>('get_web_search_config');
}

export function configureWebSearch(request: ConfigureWebSearchRequest): Promise<WebSearchConfig> {
  return invoke<WebSearchConfig>('configure_web_search', { request });
}

export function setWebSearchEnabled(enabled: boolean): Promise<WebSearchConfig> {
  return invoke<WebSearchConfig>('set_web_search_enabled', { request: { enabled } });
}

/** Remove the active provider metadata and its current OS-keychain credential. */
export function clearWebSearchConfig(): Promise<void> {
  return invoke<void>('clear_web_search_config');
}
