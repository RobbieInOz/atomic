import { useState, useRef, useEffect } from 'react';
import { Database, ChevronDown, Pencil, Trash2 } from 'lucide-react';
import { useDatabasesStore, DatabaseInfo } from '../stores/databases';

export function DatabaseSwitcher() {
  const { databases, activeId, fetchDatabases, switchDatabase, createDatabase, renameDatabase, deleteDatabase } = useDatabasesStore();
  const [isOpen, setIsOpen] = useState(false);
  const [isCreating, setIsCreating] = useState(false);
  const [newName, setNewName] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    fetchDatabases();
  }, [fetchDatabases]);

  // Close dropdown on outside click
  useEffect(() => {
    if (!isOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
        setIsCreating(false);
        setEditingId(null);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [isOpen]);

  const activeDb = databases.find(d => d.id === activeId);
  const activeName = activeDb?.name ?? 'Database';

  const handleSwitch = async (id: string) => {
    if (id === activeId) return;
    setIsOpen(false);
    await switchDatabase(id);
  };

  const handleCreate = async () => {
    if (!newName.trim()) return;
    await createDatabase(newName.trim());
    setNewName('');
    setIsCreating(false);
  };

  const handleRename = async (id: string) => {
    if (!editName.trim()) return;
    await renameDatabase(id, editName.trim());
    setEditingId(null);
  };

  const handleDelete = async (db: DatabaseInfo) => {
    if (db.is_default) return;
    if (!confirm(`Delete "${db.name}"? This cannot be undone.`)) return;
    await deleteDatabase(db.id);
  };

  if (databases.length === 0) {
    return null;
  }

  return (
    <div className="relative w-full min-w-0" ref={dropdownRef}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="w-full flex items-center gap-1.5 px-2 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] rounded transition-colors"
        title={activeName}
      >
        <Database className="w-3 h-3 flex-shrink-0 opacity-60" strokeWidth={2} />
        <span className="truncate">{activeName}</span>
        <ChevronDown className="w-2 h-2 flex-shrink-0 opacity-40" strokeWidth={2} />
      </button>

      {isOpen && (
        <div className="absolute top-full left-0 right-0 mt-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] rounded-lg shadow-xl z-50 py-1">
          {databases.map(db => (
            <div
              key={db.id}
              className={`flex items-center gap-2 px-3 py-1.5 text-xs cursor-pointer hover:bg-[var(--color-bg-hover)] ${
                db.id === activeId ? 'text-[var(--color-accent)]' : 'text-[var(--color-text-primary)]'
              }`}
            >
              {editingId === db.id ? (
                <input
                  autoFocus
                  className="flex-1 bg-transparent border border-[var(--color-border)] rounded px-1 py-0.5 text-xs outline-none"
                  value={editName}
                  onChange={e => setEditName(e.target.value)}
                  onKeyDown={e => {
                    if (e.key === 'Enter') handleRename(db.id);
                    if (e.key === 'Escape') setEditingId(null);
                  }}
                  onBlur={() => setEditingId(null)}
                />
              ) : (
                <>
                  <span
                    className="flex-1 truncate"
                    onClick={() => handleSwitch(db.id)}
                  >
                    {db.name}
                  </span>
                  <button
                    onClick={(e) => { e.stopPropagation(); setEditingId(db.id); setEditName(db.name); }}
                    className="opacity-0 group-hover:opacity-100 hover:text-[var(--color-text-primary)] text-[var(--color-text-tertiary)]"
                    title="Rename"
                  >
                    <Pencil className="w-2.5 h-2.5" strokeWidth={2} />
                  </button>
                  {!db.is_default && (
                    <button
                      onClick={(e) => { e.stopPropagation(); handleDelete(db); }}
                      className="opacity-0 group-hover:opacity-100 hover:text-red-400 text-[var(--color-text-tertiary)]"
                      title="Delete"
                    >
                      <Trash2 className="w-2.5 h-2.5" strokeWidth={2} />
                    </button>
                  )}
                </>
              )}
            </div>
          ))}

          <div className="border-t border-[var(--color-border)] mt-1 pt-1">
            {isCreating ? (
              <div className="px-3 py-1.5">
                <input
                  autoFocus
                  className="w-full bg-transparent border border-[var(--color-border)] rounded px-2 py-1 text-xs outline-none text-[var(--color-text-primary)]"
                  placeholder="Database name..."
                  value={newName}
                  onChange={e => setNewName(e.target.value)}
                  onKeyDown={e => {
                    if (e.key === 'Enter') handleCreate();
                    if (e.key === 'Escape') { setIsCreating(false); setNewName(''); }
                  }}
                />
              </div>
            ) : (
              <button
                onClick={() => setIsCreating(true)}
                className="w-full text-left px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)]"
              >
                + New database
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
