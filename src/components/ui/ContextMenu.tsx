import { ReactNode, useRef, useEffect, useState, useCallback } from 'react';
import { createPortal } from 'react-dom';

interface ContextMenuItem {
  label: string;
  onClick: () => void;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
}

interface ContextMenuProps {
  items: ContextMenuItem[];
  position: { x: number; y: number } | null;
  onClose: () => void;
  /// When true, focus the first non-disabled item on open. Callers
  /// that open the menu via a keyboard-reachable button (overflow ⋮,
  /// mobile header more-actions) should pass `true` so arrow-key
  /// navigation begins immediately. Right-click callers (TagTree)
  /// leave this `false` so the menu opens without stealing focus
  /// from wherever the user clicked.
  autoFocus?: boolean;
}

export function ContextMenu({ items, position, onClose, autoFocus = false }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [adjustedPosition, setAdjustedPosition] = useState(position);
  const [highlighted, setHighlighted] = useState(0);

  useEffect(() => {
    if (!position) return;

    // Adjust position to keep menu in viewport
    const menu = menuRef.current;
    if (menu) {
      const rect = menu.getBoundingClientRect();
      const viewportWidth = window.innerWidth;
      const viewportHeight = window.innerHeight;

      let x = position.x;
      let y = position.y;

      if (x + rect.width > viewportWidth) {
        x = viewportWidth - rect.width - 8;
      }
      if (y + rect.height > viewportHeight) {
        y = viewportHeight - rect.height - 8;
      }

      setAdjustedPosition({ x, y });
    }
  }, [position]);

  // Resolve the first non-disabled index. Used both for the
  // open-time focus and the Home key. Defaults to 0 if every item is
  // disabled (the menu shouldn't have been rendered in that case, but
  // we don't want to crash).
  const firstEnabled = useCallback(() => {
    const idx = items.findIndex((it) => !it.disabled);
    return idx >= 0 ? idx : 0;
  }, [items]);

  // On open: reset highlight + (optionally) move focus into the menu.
  // We defer to a microtask so the dropdown has actually rendered.
  useEffect(() => {
    if (!position) return;
    const initial = firstEnabled();
    setHighlighted(initial);
    if (autoFocus) {
      queueMicrotask(() => {
        itemRefs.current[initial]?.focus();
      });
    }
  }, [position, autoFocus, firstEnabled]);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };

    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };

    if (position) {
      document.addEventListener('mousedown', handleClickOutside);
      document.addEventListener('keydown', handleEscape);
    }

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleEscape);
    };
  }, [position, onClose]);

  const moveHighlight = useCallback((direction: 1 | -1) => {
    if (items.length === 0) return;
    let next = highlighted;
    // Skip over disabled items; cap iterations at items.length to
    // avoid infinite loop if every item is disabled.
    for (let i = 0; i < items.length; i++) {
      next = (next + direction + items.length) % items.length;
      if (!items[next]?.disabled) break;
    }
    setHighlighted(next);
    itemRefs.current[next]?.focus();
  }, [highlighted, items]);

  const onMenuKeyDown = useCallback((e: React.KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        moveHighlight(1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        moveHighlight(-1);
        break;
      case 'Home': {
        e.preventDefault();
        const idx = firstEnabled();
        setHighlighted(idx);
        itemRefs.current[idx]?.focus();
        break;
      }
      case 'End': {
        e.preventDefault();
        for (let i = items.length - 1; i >= 0; i--) {
          if (!items[i].disabled) {
            setHighlighted(i);
            itemRefs.current[i]?.focus();
            break;
          }
        }
        break;
      }
      // Enter / Space fall through to the button's native click handler.
    }
  }, [moveHighlight, firstEnabled, items]);

  if (!position) return null;

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      /* An open menu owns Escape and the click that closes it — the same
         claim Modal and the palettes make. Without the marker, a capture-phase
         Escape handler further out (fullscreen chat's, which stops the event
         there) closes itself and leaves this menu floating over a view it no
         longer belongs to. */
      data-modal="true"
      onKeyDown={onMenuKeyDown}
      /* The portal moves the menu's DOM to <body>, but React events still
         bubble along the component tree — so when a caller renders the menu
         inside a clickable card/row (WikiCard, ReportRow), an item click
         would also fire the card's own onClick. A menu's clicks are its own;
         they end here. */
      onClick={(e) => e.stopPropagation()}
      onAuxClick={(e) => e.stopPropagation()}
      className="fixed z-50 min-w-[160px] bg-[var(--color-bg-card)] border border-[var(--color-border)] rounded-lg shadow-xl py-1 animate-in fade-in zoom-in-95 duration-100"
      style={{
        left: adjustedPosition?.x ?? position.x,
        top: adjustedPosition?.y ?? position.y,
      }}
    >
      {items.map((item, index) => (
        <button
          key={index}
          ref={(el) => { itemRefs.current[index] = el; }}
          role="menuitem"
          tabIndex={-1}
          onClick={() => {
            if (!item.disabled) {
              item.onClick();
              onClose();
            }
          }}
          onMouseEnter={() => !item.disabled && setHighlighted(index)}
          disabled={item.disabled}
          className={`w-full px-4 py-2 text-left text-sm flex items-center gap-2 transition-colors focus:outline-none ${
            item.disabled
              ? 'text-[var(--color-text-tertiary)] cursor-not-allowed'
              : item.danger
              ? `text-red-400 ${highlighted === index ? 'bg-red-500/10' : 'hover:bg-red-500/10'}`
              : `text-[var(--color-text-primary)] ${highlighted === index ? 'bg-[var(--color-bg-hover)]' : 'hover:bg-[var(--color-bg-hover)]'}`
          }`}
        >
          {item.icon && <span className="w-4 h-4">{item.icon}</span>}
          {item.label}
        </button>
      ))}
    </div>,
    document.body
  );
}
