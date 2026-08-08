import { useEffect, useRef, useState } from 'react';
import { useWikiStore } from '../../stores/wiki';
import { useUIStore } from '../../stores/ui';
import { WikiGrid } from './WikiGrid';
import { NewWikiModal } from './NewWikiModal';
import { TagWikiPromptModal } from '../tags/TagWikiPromptModal';
import type { Tag } from '../../stores/tags';

export function WikiFullView() {
  const articles = useWikiStore(s => s.articles);
  const suggestedArticles = useWikiStore(s => s.suggestedArticles);
  const isLoadingList = useWikiStore(s => s.isLoadingList);
  const fetchAllArticles = useWikiStore(s => s.fetchAllArticles);

  const openWikiReader = useUIStore(s => s.openWikiReader);

  const [isModalOpen, setIsModalOpen] = useState(false);
  /// The tag whose wiki prompts are being edited — also the modal's open flag.
  /// The grid hands over the card's tag id and name, which is everything the
  /// editor needs; there is nothing to look up.
  const [promptTag, setPromptTag] = useState<Pick<Tag, 'id' | 'name'> | null>(null);
  const initializedRef = useRef(false);

  useEffect(() => {
    if (initializedRef.current) return;
    initializedRef.current = true;
    fetchAllArticles();
  }, [fetchAllArticles]);

  // No unmount reset: opening a reader tab unmounts this view, and wiping the
  // store there costs the sidebar list its data on every tab open/close.
  // Switching databases still resets it — see stores/databases.ts.

  const handleArticleClick = (tagId: string, tagName: string, opts?: { newTab?: boolean }) => {
    openWikiReader(tagId, tagName, undefined, opts);
  };

  const handleSuggestionClick = (tagId: string, tagName: string, opts?: { newTab?: boolean }) => {
    // Open wiki reader — it will show the empty state and allow generation
    openWikiReader(tagId, tagName, undefined, opts);
  };

  return (
    <div className="h-full overflow-hidden flex flex-col">
      <WikiGrid
        articles={articles}
        suggestedArticles={suggestedArticles}
        onArticleClick={handleArticleClick}
        onSuggestionClick={handleSuggestionClick}
        onOpenPrompts={(tagId, tagName) => setPromptTag({ id: tagId, name: tagName })}
        isLoading={isLoadingList}
      />

      {/* New Wiki Modal */}
      <NewWikiModal isOpen={isModalOpen} onClose={() => setIsModalOpen(false)} />

      {/* Wiki Prompt Modal */}
      <TagWikiPromptModal
        isOpen={promptTag !== null}
        tag={promptTag}
        onClose={() => setPromptTag(null)}
      />
    </div>
  );
}
