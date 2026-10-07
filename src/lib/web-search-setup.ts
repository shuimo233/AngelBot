export type SecureWebSearchSetup = {
  provider: string;
  apiKey: string;
  endpoint?: string;
};

/**
 * Recognise an explicit conversational setup request before the raw key can
 * reach the optimistic transcript, persistence layer, or model context.
 */
export function extractSecureWebSearchSetup(text: string): {
  displayContent: string;
  setup: SecureWebSearchSetup | null;
} {
  const match = text.trim().match(
    /^(?:配置|设置|启用)\s*(?:联网搜索|web\s*search)\s*[:：]?\s*(tavily|brave(?:_search)?|exa|searx(?:ng)?)\s+([^\s]+)$/i,
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
