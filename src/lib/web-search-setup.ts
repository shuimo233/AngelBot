export type SecureWebSearchSetup = {
  provider: string;
  apiKey: string;
  endpoint?: string;
};

const secureSetupPrefix = /^(?:配置|设置|启用)\s*(?:联网搜索|web\s*search)/i;

/** Keep the whole setup composition out of drafts while its key is being entered. */
export function isSecureWebSearchSetupDraft(text: string): boolean {
  return secureSetupPrefix.test(text.trimStart());
}

/**
 * Recognise an explicit conversational setup request before the raw key can
 * reach the optimistic transcript, persistence layer, or model context.
 */
export function extractSecureWebSearchSetup(text: string): {
  displayContent: string;
  setup: SecureWebSearchSetup | null;
} {
  const prefix = text.trimStart().match(secureSetupPrefix)?.[0];
  const match = prefix && text.trimStart().slice(prefix.length).trimEnd().match(
    /^\s*[:：]?\s*(tavily|brave(?:_search)?|exa|searx(?:ng)?)\s+([^\s]+)$/i,
  );
  if (!match) return { displayContent: text, setup: null };
  const provider = match[1].toLowerCase().replace('_search', '');
  const value = match[2];
  const isSearxng = provider === 'searxng' || provider === 'searx';
  return {
    displayContent: isSearxng
      ? '配置联网搜索（SearXNG 服务地址已安全提交）'
      : `配置联网搜索（${provider}，凭据已安全提交）`,
    setup: isSearxng
      ? { provider: 'searxng', apiKey: '', endpoint: value }
      : { provider, apiKey: value },
  };
}
