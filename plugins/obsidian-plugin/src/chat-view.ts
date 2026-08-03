import {
  ItemView,
  MarkdownRenderer,
  Notice,
  WorkspaceLeaf,
  setIcon,
} from "obsidian";
import {
  AtomicClient,
  type AtomWithTags,
  type ChatCitation,
  type ChatMessageWithContext,
  type ChatToolCall,
  type ConversationWithMessages,
  type ConversationWithTags,
  type Tag,
  type TagWithCount,
} from "./atomic-client";
import type { AtomicWebSocket, ServerEvent } from "./ws-client";

export const CHAT_VIEW_TYPE = "atomic-chat";

interface StreamingMessage {
  content: string;
  toolCalls: Map<string, ChatToolCall>;
}

export class ChatView extends ItemView {
  private client: AtomicClient;
  private ws: AtomicWebSocket;
  private getVaultName: () => string;

  private conversations: ConversationWithTags[] = [];
  private active: ConversationWithMessages | null = null;
  private allTags: TagWithCount[] = [];
  private atomCache = new Map<string, AtomWithTags>();

  private loading = false;
  private sending = false;
  private streaming: StreamingMessage | null = null;
  private conversationListOpen = false;
  private scopeEditorOpen = false;

  private unsubscribeWs: (() => void) | null = null;

  // DOM roots we update in place so streaming re-renders are cheap.
  private threadEl: HTMLElement | null = null;
  private headerEl: HTMLElement | null = null;
  private convListEl: HTMLElement | null = null;
  private inputEl: HTMLTextAreaElement | null = null;
  private sendBtn: HTMLButtonElement | null = null;

  constructor(
    leaf: WorkspaceLeaf,
    client: AtomicClient,
    ws: AtomicWebSocket,
    getVaultName: () => string
  ) {
    super(leaf);
    this.client = client;
    this.ws = ws;
    this.getVaultName = getVaultName;
  }

  getViewType(): string {
    return CHAT_VIEW_TYPE;
  }

  getDisplayText(): string {
    return "Atomic chat";
  }

  getIcon(): string {
    return "messages-square";
  }

  async onOpen(): Promise<void> {
    this.ws.open();
    this.unsubscribeWs = this.ws.on((evt) => this.handleWsEvent(evt));

    this.renderShell();
    await Promise.all([this.loadConversations(), this.loadTags()]);

    // Open the most recent conversation, or start a fresh one.
    const latest = this.conversations.find((c) => !c.is_archived);
    if (latest) {
      await this.selectConversation(latest.id);
    } else {
      await this.startNewConversation([]);
    }
  }

  onClose(): Promise<void> {
    this.unsubscribeWs?.();
    this.unsubscribeWs = null;
    return Promise.resolve();
  }

  // ------------------------------------------------------------------
  // Data loading
  // ------------------------------------------------------------------

  private async loadConversations(): Promise<void> {
    try {
      this.conversations = await this.client.listConversations();
    } catch (e) {
      console.error("[Atomic] failed to load conversations:", e);
      new Notice(`Couldn't load conversations: ${e instanceof Error ? e.message : String(e)}`);
      this.conversations = [];
    }
  }

  private async loadTags(): Promise<void> {
    try {
      this.allTags = await this.client.getTags(0);
    } catch (e) {
      console.error("[Atomic] failed to load tags:", e);
      new Notice(`Couldn't load tags: ${e instanceof Error ? e.message : String(e)}`);
      this.allTags = [];
    }
  }

  private async selectConversation(id: string): Promise<void> {
    this.loading = true;
    this.streaming = null;
    this.renderThread();
    try {
      this.active = await this.client.getConversation(id);
    } catch (e) {
      console.error("[Atomic] failed to load conversation:", e);
      new Notice("Failed to load conversation");
    } finally {
      this.loading = false;
      this.renderHeader();
      this.renderThread();
    }
  }

  private async startNewConversation(tagIds: string[]): Promise<void> {
    try {
      const created = await this.client.createConversation(tagIds);
      this.active = {
        ...created,
        messages: [],
      };
      await this.loadConversations();
    } catch (e) {
      console.error("[Atomic] failed to create conversation:", e);
      new Notice("Failed to create conversation");
      return;
    }
    this.streaming = null;
    this.conversationListOpen = false;
    this.renderHeader();
    this.renderConversationList();
    this.renderThread();
  }

  // ------------------------------------------------------------------
  // Sending + streaming
  // ------------------------------------------------------------------

  private async send(): Promise<void> {
    if (!this.active || this.sending || !this.inputEl) return;
    const content = this.inputEl.value.trim();
    if (!content) return;

    this.sending = true;
    this.streaming = { content: "", toolCalls: new Map() };

    // Optimistic user message — the server response (HTTP) contains the
    // definitive stored record, but we show the bubble immediately.
    const now = new Date().toISOString();
    this.active.messages.push({
      id: `local-${Date.now()}`,
      conversation_id: this.active.id,
      role: "user",
      content,
      created_at: now,
      message_index: this.active.messages.length,
      tool_calls: [],
      citations: [],
    });

    this.inputEl.value = "";
    this.autoResizeInput();
    this.renderThread();
    this.updateSendButton();

    try {
      // The HTTP call blocks until the agent loop finishes and returns the
      // final assistant message; the WS ChatComplete event usually arrives
      // first. Both deliver the same message — dedupe by id so only one is
      // appended.
      const finalMsg = await this.client.sendChatMessage(this.active.id, content);
      if (!this.active.messages.some((m) => m.id === finalMsg.id)) {
        this.active.messages.push(finalMsg);
      }
      this.streaming = null;
    } catch (e) {
      console.error("[Atomic] send failed:", e);
      new Notice(`Chat error: ${e instanceof Error ? e.message : String(e)}`);
      this.streaming = null;
    } finally {
      this.sending = false;
      this.renderThread();
      this.updateSendButton();
    }
  }

  private handleWsEvent(evt: ServerEvent): void {
    if (!this.active) return;
    const convId = (evt as { conversation_id?: string }).conversation_id;
    if (convId !== this.active.id) return;

    switch (evt.type) {
      case "ChatStreamDelta": {
        if (!this.streaming) this.streaming = { content: "", toolCalls: new Map() };
        this.streaming.content += (evt as { content: string }).content;
        this.renderStreamingBubble();
        break;
      }
      case "ChatToolStart": {
        if (!this.streaming) this.streaming = { content: "", toolCalls: new Map() };
        const e = evt as {
          tool_call_id: string;
          tool_name: string;
          tool_input: unknown;
        };
        this.streaming.toolCalls.set(e.tool_call_id, {
          id: e.tool_call_id,
          message_id: "",
          tool_name: e.tool_name,
          tool_input: e.tool_input,
          tool_output: null,
          status: "running",
          created_at: new Date().toISOString(),
          completed_at: null,
        });
        this.renderStreamingBubble();
        break;
      }
      case "ChatToolComplete": {
        const e = evt as { tool_call_id: string; results_count: number };
        const existing = this.streaming?.toolCalls.get(e.tool_call_id);
        if (existing) {
          existing.status = "complete";
          existing.tool_output = { results_count: e.results_count };
          existing.completed_at = new Date().toISOString();
        }
        this.renderStreamingBubble();
        break;
      }
      case "ChatComplete": {
        const e = evt as { message: ChatMessageWithContext };
        if (this.active && !this.active.messages.some((m) => m.id === e.message.id)) {
          this.active.messages.push(e.message);
        }
        this.streaming = null;
        this.renderThread();
        break;
      }
      case "ChatError": {
        const e = evt as { error: string };
        new Notice(`Chat error: ${e.error}`);
        this.streaming = null;
        this.renderThread();
        break;
      }
    }
  }

  // ------------------------------------------------------------------
  // Rendering
  // ------------------------------------------------------------------

  private renderShell(): void {
    const container = this.containerEl.children[1];
    container.empty();
    container.addClass("atomic-chat-root");

    this.headerEl = container.createDiv({ cls: "atomic-chat-header" });
    this.convListEl = container.createDiv({ cls: "atomic-chat-conv-list hidden" });
    this.threadEl = container.createDiv({ cls: "atomic-chat-thread" });

    const inputWrap = container.createDiv({ cls: "atomic-chat-input-wrap" });
    this.inputEl = inputWrap.createEl("textarea", {
      cls: "atomic-chat-input",
      attr: { placeholder: "Ask your knowledge base…", rows: "1" },
    });
    this.inputEl.addEventListener("input", () => {
      this.autoResizeInput();
      this.updateSendButton();
    });
    this.inputEl.addEventListener("keydown", (evt) => {
      if (evt.key === "Enter" && !evt.shiftKey && !evt.isComposing) {
        evt.preventDefault();
        void this.send();
      }
    });

    this.sendBtn = inputWrap.createEl("button", {
      cls: "atomic-chat-send",
      attr: { "aria-label": "Send" },
    });
    setIcon(this.sendBtn, "send-horizontal");
    this.sendBtn.addEventListener("click", () => void this.send());

    this.renderHeader();
  }

  private renderHeader(): void {
    if (!this.headerEl) return;
    this.headerEl.empty();

    const title = this.headerEl.createDiv({ cls: "atomic-chat-title" });
    title.setText(this.conversationTitle());

    const actions = this.headerEl.createDiv({ cls: "atomic-chat-actions" });

    const scopeChip = actions.createEl("button", { cls: "atomic-chat-scope-chip" });
    scopeChip.setText(this.scopeLabel());
    scopeChip.addEventListener("click", () => this.toggleScopeEditor(scopeChip));

    const listBtn = actions.createEl("button", {
      cls: "atomic-chat-icon-btn",
      attr: { "aria-label": "Conversations" },
    });
    setIcon(listBtn, "list");
    listBtn.addEventListener("click", () => {
      this.conversationListOpen = !this.conversationListOpen;
      this.renderConversationList();
    });

    const newBtn = actions.createEl("button", {
      cls: "atomic-chat-icon-btn",
      attr: { "aria-label": "New conversation" },
    });
    setIcon(newBtn, "plus");
    newBtn.addEventListener("click", () => void this.startNewConversation([]));
  }

  private conversationTitle(): string {
    if (!this.active) return "Atomic chat";
    if (this.active.title) return this.active.title;
    const firstUser = this.active.messages.find((m) => m.role === "user");
    if (firstUser) {
      const snippet = firstUser.content.slice(0, 40);
      return snippet + (firstUser.content.length > 40 ? "…" : "");
    }
    return "New conversation";
  }

  private scopeLabel(): string {
    if (!this.active || this.active.tags.length === 0) return "All notes";
    if (this.active.tags.length === 1) return `#${this.active.tags[0].name}`;
    return `${this.active.tags.length} tags`;
  }

  private renderConversationList(): void {
    if (!this.convListEl) return;
    this.convListEl.empty();
    this.convListEl.toggleClass("hidden", !this.conversationListOpen);
    if (!this.conversationListOpen) return;

    if (this.conversations.length === 0) {
      this.convListEl.createDiv({
        cls: "atomic-chat-conv-empty",
        text: "No conversations yet.",
      });
      return;
    }

    for (const conv of this.conversations) {
      if (conv.is_archived) continue;
      const row = this.convListEl.createDiv({ cls: "atomic-chat-conv-row" });
      if (this.active?.id === conv.id) row.addClass("active");
      const title = conv.title || conv.last_message_preview || "New conversation";
      row.createDiv({ cls: "atomic-chat-conv-title", text: title });
      if (conv.tags.length > 0) {
        const tagsEl = row.createDiv({ cls: "atomic-chat-conv-tags" });
        tagsEl.setText(conv.tags.map((t) => `#${t.name}`).join(" "));
      }
      row.addEventListener("click", () => {
        this.conversationListOpen = false;
        this.renderConversationList();
        void this.selectConversation(conv.id);
      });
    }
  }

  private toggleScopeEditor(anchor: HTMLElement): void {
    // Remove any existing popover.
    const existing = this.containerEl.querySelector(".atomic-chat-scope-popover");
    if (existing) {
      existing.remove();
      this.scopeEditorOpen = false;
      return;
    }
    this.scopeEditorOpen = true;

    const popover = this.containerEl.createDiv({ cls: "atomic-chat-scope-popover" });
    const rect = anchor.getBoundingClientRect();
    const rootRect = this.containerEl.getBoundingClientRect();
    popover.style.top = `${rect.bottom - rootRect.top + 4}px`;
    popover.style.right = `${rootRect.right - rect.right}px`;

    const currentTags = new Map<string, Tag>(
      this.active?.tags.map((t) => [t.id, t]) ?? []
    );

    const chipsEl = popover.createDiv({ cls: "atomic-chat-scope-chips" });
    const renderChips = () => {
      chipsEl.empty();
      for (const tag of currentTags.values()) {
        const chip = chipsEl.createSpan({ cls: "atomic-chat-scope-pill" });
        chip.setText(`#${tag.name}`);
        const x = chip.createEl("button", { cls: "atomic-chat-scope-remove", text: "×" });
        x.addEventListener("click", () => {
          currentTags.delete(tag.id);
          renderChips();
          void this.commitScope([...currentTags.keys()]);
        });
      }
      if (currentTags.size === 0) {
        chipsEl.createDiv({
          cls: "atomic-chat-scope-empty",
          text: "No tag filter (search all notes).",
        });
      }
    };
    renderChips();

    const input = popover.createEl("input", {
      cls: "atomic-chat-scope-input",
      attr: { placeholder: "Type a tag name…", type: "text" },
    });
    const suggestions = popover.createDiv({ cls: "atomic-chat-scope-suggestions" });

    const flatTags = this.flatTags();
    const updateSuggestions = () => {
      suggestions.empty();
      const q = input.value.trim().toLowerCase();
      if (!q) return;
      const matches = flatTags
        .filter((t) => t.name.toLowerCase().includes(q) && !currentTags.has(t.id))
        .slice(0, 8);
      for (const tag of matches) {
        const row = suggestions.createDiv({ cls: "atomic-chat-scope-suggest-row" });
        row.setText(`#${tag.name}`);
        row.addEventListener("click", () => {
          currentTags.set(tag.id, {
            id: tag.id,
            name: tag.name,
            parent_id: tag.parent_id,
            created_at: tag.created_at,
          });
          input.value = "";
          updateSuggestions();
          renderChips();
          void this.commitScope([...currentTags.keys()]);
        });
      }
    };
    input.addEventListener("input", updateSuggestions);
    input.focus();

    const doc = activeDocument;
    const close = (evt: MouseEvent) => {
      if (!popover.contains(evt.target as Node) && evt.target !== anchor) {
        popover.remove();
        this.scopeEditorOpen = false;
        doc.removeEventListener("mousedown", close, true);
      }
    };
    doc.addEventListener("mousedown", close, true);
  }

  private flatTags(): TagWithCount[] {
    const out: TagWithCount[] = [];
    const walk = (nodes: TagWithCount[]) => {
      for (const n of nodes) {
        // Hide autotag category roots (Topics, People, …) — real tags are
        // their children. Matches the filtering we do in the wiki view.
        if (!n.is_autotag_target) out.push(n);
        if (n.children.length > 0) walk(n.children);
      }
    };
    walk(this.allTags);
    return out;
  }

  private async commitScope(tagIds: string[]): Promise<void> {
    if (!this.active) return;
    try {
      const updated = await this.client.setConversationScope(this.active.id, tagIds);
      this.active.tags = updated.tags;
      this.renderHeader();
    } catch (e) {
      console.error("[Atomic] failed to update scope:", e);
      new Notice("Failed to update scope");
    }
  }

  private renderThread(): void {
    if (!this.threadEl) return;
    this.threadEl.empty();

    if (this.loading) {
      this.threadEl.createDiv({ cls: "atomic-chat-loading", text: "Loading…" });
      return;
    }

    if (!this.active) return;

    if (this.active.messages.length === 0 && !this.streaming) {
      this.threadEl.createDiv({
        cls: "atomic-chat-empty",
        text: "Ask a question — I'll search your notes for context.",
      });
      return;
    }

    for (const msg of this.active.messages) {
      this.renderMessage(msg);
    }
    if (this.streaming) {
      this.renderStreamingBubble();
    }
    this.scrollToBottom();
  }

  private renderMessage(msg: ChatMessageWithContext): void {
    if (!this.threadEl) return;
    const bubble = this.threadEl.createDiv({
      cls: `atomic-chat-msg atomic-chat-msg-${msg.role}`,
    });

    if (msg.tool_calls.length > 0) {
      this.renderToolCalls(bubble, msg.tool_calls);
    }

    const body = bubble.createDiv({ cls: "atomic-chat-msg-body" });
    const rewritten = this.rewriteCitations(msg.content, msg.citations);
    const sourcePath = this.app.workspace.getActiveFile()?.path ?? "";
    void MarkdownRenderer.render(this.app, rewritten, body, sourcePath, this);
    this.wireLinkClicks(body, sourcePath, msg.citations);
  }

  private renderStreamingBubble(): void {
    if (!this.threadEl || !this.streaming) return;
    // Remove any prior streaming bubble and re-add at the end.
    this.threadEl.querySelectorAll(".atomic-chat-msg-streaming").forEach((el) => el.remove());

    const bubble = this.threadEl.createDiv({
      cls: "atomic-chat-msg atomic-chat-msg-assistant atomic-chat-msg-streaming",
    });

    if (this.streaming.toolCalls.size > 0) {
      this.renderToolCalls(bubble, [...this.streaming.toolCalls.values()]);
    }

    const body = bubble.createDiv({ cls: "atomic-chat-msg-body" });
    if (this.streaming.content) {
      void MarkdownRenderer.render(
        this.app,
        this.streaming.content,
        body,
        this.app.workspace.getActiveFile()?.path ?? "",
        this
      );
    } else {
      body.createDiv({ cls: "atomic-chat-typing" }).setText("•••");
    }
    this.scrollToBottom();
  }

  private renderToolCalls(parent: HTMLElement, calls: ChatToolCall[]): void {
    const wrap = parent.createDiv({ cls: "atomic-chat-tool-calls" });
    for (const call of calls) {
      const details = wrap.createEl("details", { cls: "atomic-chat-tool-call" });
      details.addClass(`status-${call.status}`);
      const summary = details.createEl("summary");
      const icon = summary.createSpan({ cls: "atomic-chat-tool-icon" });
      setIcon(icon, call.status === "running" ? "loader" : "wrench");
      summary.createSpan({ cls: "atomic-chat-tool-name", text: call.tool_name });
      summary.createSpan({
        cls: "atomic-chat-tool-status",
        text:
          call.status === "running"
            ? "running…"
            : call.status === "complete"
            ? "done"
            : call.status,
      });
      const body = details.createDiv({ cls: "atomic-chat-tool-body" });
      body.createEl("pre", {
        cls: "atomic-chat-tool-input",
        text: JSON.stringify(call.tool_input, null, 2),
      });
      if (call.tool_output !== null && call.tool_output !== undefined) {
        body.createEl("pre", {
          cls: "atomic-chat-tool-output",
          text: JSON.stringify(call.tool_output, null, 2),
        });
      }
    }
  }

  /**
   * Same citation strategy as the wiki view: `[N]` → `[[vault-path|N]]` when
   * the cited atom's source URL points into this vault, else left as plain
   * text. Chat citations don't include source_url on the wire, so we look up
   * atoms lazily and rewrite what we already have cached. Uncached citations
   * become clickable inline markers that fetch on demand.
   */
  private rewriteCitations(content: string, citations: ChatCitation[]): string {
    const byIndex = new Map<number, ChatCitation>();
    for (const c of citations) byIndex.set(c.citation_index, c);
    const vaultPrefix = `obsidian://${this.getVaultName()}/`;

    return content.replace(/\[(\d+)\]/g, (match, idxStr: string) => {
      const index = parseInt(idxStr, 10);
      const cit = byIndex.get(index);
      if (!cit) return match;
      // Wiki articles and findings have no vault note and no atom to fetch;
      // their marker stays plain text rather than becoming a dead link.
      if (cit.source_type && cit.source_type !== "atom") return match;

      const atom = this.atomCache.get(cit.atom_id);
      const url = atom?.source_url ?? null;
      if (url && url.startsWith(vaultPrefix)) {
        try {
          const encoded = url.slice(vaultPrefix.length);
          const path = encoded
            .split("/")
            .map((seg) => decodeURIComponent(seg))
            .join("/")
            .replace(/\.md$/i, "");
          return `[[${path}|${index}]]`;
        } catch {
          return match;
        }
      }
      // Not cached (or not a vault atom) — render as a sentinel marker that
      // our click handler can recognize. We use a Markdown footnote-style
      // anchor so react-markdown renders it as a link we can intercept.
      return `[${index}](atomic-citation:${cit.atom_id})`;
    });
  }

  private wireLinkClicks(
    root: HTMLElement,
    sourcePath: string,
    citations: ChatCitation[]
  ): void {
    root.addEventListener(
      "click",
      (evt) => { void this.onLinkClick(evt, sourcePath, citations); },
      true
    );
  }

  private async onLinkClick(evt: MouseEvent, sourcePath: string, citations: ChatCitation[]): Promise<void> {
    const target = evt.target as HTMLElement | null;
    const link = target?.closest("a") as HTMLAnchorElement | null;
    if (!link) return;
    evt.preventDefault();
    evt.stopPropagation();

    const href = link.getAttribute("data-href") ?? link.getAttribute("href") ?? "";

    // Uncached citation marker — fetch atom, cache, then either open the
    // vault note or show the excerpt.
    if (href.startsWith("atomic-citation:")) {
      const atomId = href.slice("atomic-citation:".length);
      await this.handleCitationClick(atomId, citations);
      return;
    }

    // Cross-wiki reference — switch the wiki view isn't available here,
    // so fall back to Obsidian's link resolution, which will treat it as
    // a vault note if one exists.
    const newLeaf = evt.ctrlKey || evt.metaKey;
    await this.app.workspace.openLinkText(href, sourcePath, newLeaf);
  }

  private async handleCitationClick(atomId: string, citations: ChatCitation[]): Promise<void> {
    let atom = this.atomCache.get(atomId);
    if (!atom) {
      try {
        atom = await this.client.getAtom(atomId);
        this.atomCache.set(atomId, atom);
      } catch (e) {
        console.error("[Atomic] failed to fetch cited atom:", e);
        new Notice("Could not fetch cited atom");
        return;
      }
    }

    const vaultPrefix = `obsidian://${this.getVaultName()}/`;
    if (atom.source_url && atom.source_url.startsWith(vaultPrefix)) {
      try {
        const path = atom.source_url
          .slice(vaultPrefix.length)
          .split("/")
          .map((s) => decodeURIComponent(s))
          .join("/");
        await this.app.workspace.openLinkText(path, "", false);
      } catch (e) {
        console.error("[Atomic] open note failed:", e);
      }
      return;
    }

    const cit = citations.find((c) => c.atom_id === atomId);
    const excerpt = cit?.excerpt ?? atom.snippet ?? "";
    new Notice(
      `${atom.title || "Cited atom"}\n\n${excerpt.slice(0, 200)}${
        excerpt.length > 200 ? "…" : ""
      }`,
      8000
    );
  }

  private autoResizeInput(): void {
    if (!this.inputEl) return;
    this.inputEl.setCssStyles({ height: "auto" });
    this.inputEl.setCssStyles({ height: `${Math.min(this.inputEl.scrollHeight, 200)}px` });
  }

  private updateSendButton(): void {
    if (!this.sendBtn || !this.inputEl) return;
    const hasText = this.inputEl.value.trim().length > 0;
    this.sendBtn.disabled = !hasText || this.sending;
  }

  private scrollToBottom(): void {
    if (!this.threadEl) return;
    this.threadEl.scrollTop = this.threadEl.scrollHeight;
  }
}
