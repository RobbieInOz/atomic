import { Settings } from 'lucide-react';

interface SettingsButtonProps {
  onClick: () => void;
}

export function SettingsButton({ onClick }: SettingsButtonProps) {
  return (
    <button
      onClick={onClick}
      className="p-1.5 shrink-0 text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded-md transition-colors"
      title="Settings"
    >
      <Settings className="w-4 h-4" strokeWidth={2} />
    </button>
  );
}

