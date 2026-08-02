import { EditorView, ViewPlugin } from '@codemirror/view';
import type { Extension } from '@codemirror/state';
import type { MutableRefObject } from 'react';

export type TocEditorViewRef = MutableRefObject<EditorView | null>;

/**
 * Publish the mounted `EditorView` into `viewRef` so code outside the editor
 * can drive it by document offset.
 *
 * The editor captures its `extensions` prop once at mount (keyed on
 * `documentId`), so the ref identity must be stable — pass a `useRef` value,
 * never a freshly built object.
 */
export function tocViewExtension(viewRef: TocEditorViewRef): Extension {
  return ViewPlugin.define((view) => {
    viewRef.current = view;
    return {
      destroy() {
        // Guard against clobbering a newer view: on a document swap the next
        // editor's plugin is constructed before this one is destroyed.
        if (viewRef.current === view) viewRef.current = null;
      },
    };
  });
}

/**
 * Scroll a document offset to the top of the viewport.
 *
 * CM6 only renders the lines near the viewport, so a DOM query for a heading
 * far down a long note finds nothing. Going through `scrollIntoView` lets the
 * editor resolve the position against its own height map and land exactly,
 * measured or not.
 */
export function scrollToOffset(view: EditorView, offset: number, yMargin = 72): void {
  const pos = Math.max(0, Math.min(offset, view.state.doc.length));
  view.dispatch({
    effects: EditorView.scrollIntoView(pos, { y: 'start', yMargin }),
  });
}
