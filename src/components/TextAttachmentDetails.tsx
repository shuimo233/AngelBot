import { useState } from 'react';
import type { TextAttachment } from '$types';
import { textAttachmentBytes } from '$lib/text-attachments';
import './TextAttachmentDetails.css';

/** The exact immutable snapshot, before sending or when reading history. */
export function TextAttachmentDetails({ file }: { file: TextAttachment }) {
  const [expanded, setExpanded] = useState<TextAttachment | null>(null);
  const extension = file.name.split('.').at(-1)?.toLowerCase();
  const kind = extension === 'docx' ? 'DOCX 正文文字' : extension === 'xlsx' ? 'XLSX 单元格值' : '文本快照';
  return (
    <details className="text-attachment-details" open={expanded === file}
      onToggle={(event) => setExpanded(event.currentTarget.open ? file : null)}>
      <summary><span title={file.name}>{file.name}</span><small>{textAttachmentBytes(file.text)} B · {kind}</small></summary>
      <pre tabIndex={0} aria-label={`附件文字 ${file.name}`}>{file.text}</pre>
    </details>
  );
}
