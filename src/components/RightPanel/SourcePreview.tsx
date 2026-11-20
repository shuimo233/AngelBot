import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { vs, vscDarkPlus } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { useThemeStore } from '$stores/theme';

const languages: Record<string, string> = {
  ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx', py: 'python',
  rs: 'rust', go: 'go', html: 'markup', css: 'css', sh: 'bash', ps1: 'powershell',
  bat: 'batch', sql: 'sql', json: 'json', yaml: 'yaml', yml: 'yaml', toml: 'toml', xml: 'markup',
};

const MAX_HIGHLIGHT_LENGTH = 80_000;
const MAX_HIGHLIGHT_LINES = 2_000;

export function SourcePreview({ path, content }: { path: string; content: string }) {
  const dark = useThemeStore(state => state.resolvedTheme === 'dark');
  const extension = /\.([a-z\d]+)$/i.exec(path)?.[1].toLowerCase() ?? '';
  const language = Object.prototype.hasOwnProperty.call(languages, extension) ? languages[extension] : undefined;
  if (!language || content.length > MAX_HIGHLIGHT_LENGTH || content.split('\n', MAX_HIGHLIGHT_LINES + 1).length > MAX_HIGHLIGHT_LINES) {
    return <pre className="file-preview-content" tabIndex={0} aria-label="文件原文"><code>{content}</code></pre>;
  }

  return (
    <SyntaxHighlighter
      className="file-preview-content file-preview-source"
      language={language}
      style={dark ? vscDarkPlus : vs}
      tabIndex={0}
      aria-label="文件源码"
      showLineNumbers
      showInlineLineNumbers={false}
      lineNumberContainerStyle={{ float: 'left', paddingRight: '1.25em', userSelect: 'none' }}
      lineNumberStyle={{ color: 'var(--color-muted)' }}
      customStyle={{ padding: '12px', margin: 0, fontSize: '13px', lineHeight: 1.6 }}
    >
      {content}
    </SyntaxHighlighter>
  );
}
