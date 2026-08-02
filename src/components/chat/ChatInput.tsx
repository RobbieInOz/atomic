import { ArrowUp, Square } from 'lucide-react';
import { isDemoInstance, demoSignupUrl } from '../../lib/transport';

interface ChatInputProps {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  onKeyDown: (e: React.KeyboardEvent) => void;
  disabled?: boolean;
  placeholder?: string;
  /** A response is streaming: the send button becomes a stop button. */
  isStreaming?: boolean;
  /** A stop has been requested and we're waiting for the turn to wind down. */
  isCancelling?: boolean;
  onStop?: () => void;
}

export function ChatInput({
  value,
  onChange,
  onSend,
  onKeyDown,
  disabled = false,
  placeholder = 'Type a message...',
  isStreaming = false,
  isCancelling = false,
  onStop,
}: ChatInputProps) {
  // On the public demo, chat is the signup carrot: conversations are
  // account-scoped (anonymous chat would be a shared wall), so the input
  // becomes the CTA. Single chokepoint — every chat surface renders
  // through this component.
  if (isDemoInstance()) {
    return (
      <div className="flex-shrink-0 p-3 border-t border-[var(--color-border)]">
        <a
          href={demoSignupUrl()}
          className="block w-full text-center px-4 py-3 rounded-lg border border-dashed border-[var(--color-border)] text-sm text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] hover:text-[var(--color-text-primary)] transition-colors"
        >
          Chat with your knowledge base — free on your own Atomic →
        </a>
      </div>
    );
  }
  const canSend = value.trim().length > 0 && !disabled;

  return (
    <div className="flex-shrink-0 p-3 border-t border-[var(--color-border)]">
      <div className="relative">
        <textarea
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          disabled={disabled}
          rows={1}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          className="w-full resize-none overflow-hidden bg-[var(--color-bg-main)] border border-[var(--color-border)] rounded-lg pl-4 pr-14 py-3 text-[var(--color-text-primary)] placeholder-[var(--color-text-tertiary)] focus:outline-none focus:border-[var(--color-accent)] disabled:opacity-50 disabled:cursor-not-allowed"
          style={{
            minHeight: '44px',
            maxHeight: '200px',
          }}
          onInput={(e) => {
            // Auto-resize textarea
            const target = e.target as HTMLTextAreaElement;
            target.style.height = 'auto';
            target.style.height = `${Math.min(target.scrollHeight, 200)}px`;
          }}
        />
        {isStreaming ? (
          <button
            type="button"
            onClick={onStop}
            disabled={isCancelling || !onStop}
            title={isCancelling ? 'Stopping…' : 'Stop response'}
            aria-label={isCancelling ? 'Stopping response' : 'Stop response'}
            className="absolute right-2 bottom-2 w-8 h-8 flex items-center justify-center rounded-md bg-[var(--color-bg-card)] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:border-[var(--color-accent)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          >
            <Square className="w-3.5 h-3.5 fill-current" />
          </button>
        ) : (
          <button
            type="button"
            onClick={onSend}
            disabled={!canSend}
            title="Send message"
            aria-label="Send message"
            className="absolute right-2 bottom-2 w-8 h-8 flex items-center justify-center rounded-md bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            <ArrowUp className="w-4 h-4" strokeWidth={2.5} />
          </button>
        )}
      </div>
    </div>
  );
}
