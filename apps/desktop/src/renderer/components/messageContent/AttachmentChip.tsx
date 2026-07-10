import { FileText, Image as ImageIcon } from "lucide-react";

import { MessageAttachment } from "../../types";
import { MediaToken } from "./mediaSegments";

type AttachmentChipProps = {
  attachment: MessageAttachment;
};

type MediaFileChipProps = {
  token: MediaToken;
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function AttachmentChip({ attachment }: AttachmentChipProps): JSX.Element {
  const imageSrc = attachment.dataUrl ?? attachment.url;
  const isImage = attachment.kind === "image" && imageSrc;

  return (
    <span className={`attachment-chip attachment-chip-${attachment.kind}`}>
      {isImage ? (
        <span className="attachment-chip-thumb" aria-hidden="true">
          <img src={imageSrc} alt="" />
        </span>
      ) : (
        <FileText size={14} aria-hidden="true" />
      )}
      <span className="attachment-chip-name">{attachment.name}</span>
      <span className="attachment-chip-meta">{formatBytes(attachment.size)}</span>
    </span>
  );
}

export function MediaFileChip({ token }: MediaFileChipProps): JSX.Element {
  return (
    <a
      className="chat-media-file"
      href={token.isUrl ? token.src : undefined}
      rel="noreferrer"
      target={token.isUrl ? "_blank" : undefined}
      title={token.src}
    >
      {token.isImage ? (
        <ImageIcon size={14} aria-hidden="true" />
      ) : (
        <FileText size={14} aria-hidden="true" />
      )}
      <span>{token.name}</span>
    </a>
  );
}
