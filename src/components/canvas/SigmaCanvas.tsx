import { useEffect, useRef, useState, useCallback } from 'react';
import { Loader2 } from 'lucide-react';
import { useUIStore } from '../../stores/ui';
import { useDatabasesStore } from '../../stores/databases';
import { getGlobalCanvas, type GlobalCanvasData } from '../../lib/api';
import { getTransport } from '../../lib/transport';
import Graph from 'graphology';
import Sigma from 'sigma';
import EdgeCurveProgram from '@sigma/edge-curve';
import {
  CANVAS_THEMES,
  DEFAULT_THEME,
  nodeColor,
  edgeColor,
  type CanvasTheme,
} from './sigma/themes';
import { AtomPreviewPopover } from './AtomPreviewPopover';
import { useCanvasStore } from '../../stores/canvas';

function truncLabel(str: string, max: number): string {
  return str.length > max ? str.substring(0, max - 1) + '\u2026' : str;
}

function parseRgbColor(s: string): [number, number, number] | null {
  const m = s.match(/^rgb\((\d+)\s*,\s*(\d+)\s*,\s*(\d+)\)$/);
  if (!m) return null;
  return [+m[1], +m[2], +m[3]];
}

export type SigmaCanvasMode = 'main' | 'preview';

interface SigmaCanvasProps {
  /** 'main' runs the full interactive canvas; 'preview' renders a thumbnail
   *  with no chrome and no chat controller. Static by default — pass
   *  filterAtomIds to make it interactive (pan/zoom + clickable nodes). */
  mode?: SigmaCanvasMode;
  /** Click handler for static preview mode — fires on any click inside the
   *  container. Ignored when filterAtomIds is set (the canvas is interactive
   *  and clicks are handled per-node via onPreviewNodeClick). */
  onPreviewClick?: () => void;
  /** Subset the rendered graph to these atoms + their 1-hop neighbors from
   *  the existing edge set. When provided in preview mode, the canvas
   *  becomes interactive (pan/zoom + clickable nodes). */
  filterAtomIds?: string[];
  /** Click handler for nodes in interactive preview mode. */
  onPreviewNodeClick?: (atomId: string) => void;
}

export function SigmaCanvas({
  mode = 'main',
  onPreviewClick,
  filterAtomIds,
  onPreviewNodeClick,
}: SigmaCanvasProps = {}) {
  const isPreview = mode === 'preview';
  const isInteractivePreview = isPreview && filterAtomIds !== undefined;
  // Stable key for filterAtomIds — array identity changes shouldn't rebuild.
  const filterKey = filterAtomIds ? [...filterAtomIds].sort().join(',') : null;
  // Refs let event handlers see the latest callback without re-running the
  // graph-build effect (which would tear down and rebuild Sigma).
  const onPreviewNodeClickRef = useRef(onPreviewNodeClick);
  onPreviewNodeClickRef.current = onPreviewNodeClick;
  const openReader = useUIStore(s => s.openReader);
  const selectedTagId = useUIStore(s => s.selectedTagId);
  const activeDbId = useDatabasesStore(s => s.activeId);
  const containerRef = useRef<HTMLDivElement>(null);
  // The hover pill renders into this div, which lives outside the
  // overflow-hidden Sigma container. That lets long titles spill past the
  // canvas edges instead of getting clipped (the canvas's drawing surface
  // is exactly the panel size — flipping sides only helps when the label
  // is narrower than the longer-side gap).
  const hoverPillRef = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma | null>(null);
  const graphRef = useRef<Graph | null>(null);
  const [data, setData] = useState<GlobalCanvasData | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [theme, setTheme] = useState<CanvasTheme>(DEFAULT_THEME);
  const [themePickerOpen, setThemePickerOpen] = useState(false);
  const [edgeThreshold, setEdgeThreshold] = useState(0);
  const edgeThresholdRef = useRef(0);
  const edgeAnimProgress = useRef(0); // 0 = invisible, 1 = fully visible
  const themeRef = useRef(theme);
  themeRef.current = theme;

  // Hover emphasis: when a node is hovered, dim everything outside its neighborhood.
  // neighborsRef lets the edge/node reducers answer "is X a neighbor of hovered?" in O(1).
  const hoveredNodeRef = useRef<string | null>(null);
  const neighborsRef = useRef<Map<string, Set<string>>>(new Map());
  // hoverAnim: 0 = no emphasis, 1 = fully emphasized. Reducers interpolate
  // against this so the enter/leave transitions fade instead of snapping.
  const hoverAnimRef = useRef(0);
  const hoverTargetRef = useRef(0);
  // Pinned node: the node whose popover is open. Its outline persists while
  // the popover is open so hovering other nodes still shows their titles.
  const pinnedNodeRef = useRef<string | null>(null);
  // Lifted out of the sigma useEffect so pinNode (defined before main-mode
  // branch) can kick the hover-fade animation loop.
  const startHoverAnimRef = useRef<() => void>(() => {});

  // Atom preview popover state
  const [previewAtomId, setPreviewAtomId] = useState<string | null>(null);
  const [previewAnchorRect, setPreviewAnchorRect] = useState<{ top: number; left: number; bottom: number; width: number } | null>(null);

  const closePreview = useCallback(() => {
    setPreviewAtomId(null);
    setPreviewAnchorRect(null);
    pinnedNodeRef.current = null;
    // Redraw so the pinned ring disappears. Hover state is already correct
    // because enterNode/leaveNode have been running normally all along.
    sigmaRef.current?.refresh();
  }, []);

  // Build a set of atom IDs that match the selected tag
  const selectedTagRef = useRef(selectedTagId);
  selectedTagRef.current = selectedTagId;

  // Fetch global canvas data
  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);
    setError(null);

    getGlobalCanvas()
      .then((result) => {
        if (!cancelled) {
          setData(result);
          setIsLoading(false);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err.message || 'Failed to load canvas');
          setIsLoading(false);
        }
      });

    return () => { cancelled = true; };
  }, [activeDbId]);

  // Precomputed data for the graph
  const graphDataRef = useRef<{
    edgeCounts: Map<string, number>;
    maxEdges: number;
  } | null>(null);

  // Create Sigma graph when data is loaded
  useEffect(() => {
    const container = containerRef.current;
    if (!container || !data || data.atoms.length === 0) return;

    // For an interactive preview, narrow the graph to the seed atoms plus
    // their 1-hop neighbors from the existing edge set. We reuse the global
    // PCA positions so the subset lands in the same region of space the user
    // would see if they zoomed there in the main canvas — the camera then
    // fits-to-bbox over just those nodes.
    let atoms = data.atoms;
    let edges = data.edges;
    if (filterAtomIds && filterAtomIds.length > 0) {
      const seeds = new Set(filterAtomIds);
      const included = new Set(seeds);
      for (const edge of data.edges) {
        if (seeds.has(edge.source)) included.add(edge.target);
        else if (seeds.has(edge.target)) included.add(edge.source);
      }
      atoms = data.atoms.filter(a => included.has(a.atom_id));
      edges = data.edges.filter(e => included.has(e.source) && included.has(e.target));
    }
    if (atoms.length === 0) return;

    if (sigmaRef.current) {
      sigmaRef.current.kill();
      sigmaRef.current = null;
    }

    const graph = new Graph();
    graphRef.current = graph;
    const scale = 500;

    // Compute per-atom edge count (over the rendered subset, if filtered)
    const edgeCounts = new Map<string, number>();
    for (const edge of edges) {
      edgeCounts.set(edge.source, (edgeCounts.get(edge.source) || 0) + 1);
      edgeCounts.set(edge.target, (edgeCounts.get(edge.target) || 0) + 1);
    }
    const maxEdges = Math.max(1, ...edgeCounts.values());
    graphDataRef.current = { edgeCounts, maxEdges };

    // Build atom → cluster index map
    const atomCluster = new Map<string, number>();
    for (let i = 0; i < data.clusters.length; i++) {
      for (const atomId of data.clusters[i].atom_ids) {
        atomCluster.set(atomId, i);
      }
    }

    // Add atom nodes at center — will animate to PCA positions
    const targetPositions: Record<string, { x: number; y: number }> = {};
    for (const atom of atoms) {
      const connectivity = (edgeCounts.get(atom.atom_id) || 0) / maxEdges;
      const clusterIdx = atomCluster.get(atom.atom_id);
      targetPositions[atom.atom_id] = { x: atom.x * scale, y: atom.y * scale };
      graph.addNode(atom.atom_id, {
        x: 0,
        y: 0,
        size: 2.5 + connectivity * 5,
        color: nodeColor(theme, connectivity, clusterIdx),
        label: truncLabel(atom.title || atom.atom_id.substring(0, 8), 30),
        fullLabel: atom.title || atom.atom_id.substring(0, 8),
        connectivity,
        clusterIndex: clusterIdx,
        tagIds: atom.tag_ids,
      });
    }

    // Add edges
    let minW = 1, maxW = 0;
    for (const edge of edges) {
      if (edge.weight < minW) minW = edge.weight;
      if (edge.weight > maxW) maxW = edge.weight;
    }
    const wRange = Math.max(maxW - minW, 0.001);

    const neighbors = new Map<string, Set<string>>();
    for (const edge of edges) {
      if (!graph.hasNode(edge.source) || !graph.hasNode(edge.target)) continue;
      if (graph.hasEdge(edge.source, edge.target) || graph.hasEdge(edge.target, edge.source)) continue;
      const w = (edge.weight - minW) / wRange;
      graph.addEdge(edge.source, edge.target, {
        weight: w,
        type: 'curved',
      });
      if (!neighbors.has(edge.source)) neighbors.set(edge.source, new Set());
      if (!neighbors.has(edge.target)) neighbors.set(edge.target, new Set());
      neighbors.get(edge.source)!.add(edge.target);
      neighbors.get(edge.target)!.add(edge.source);
    }
    neighborsRef.current = neighbors;

    const sigma = new Sigma(graph, container, {
      // Atom labels are drawn manually on the overlay canvas (drawLabels) with
      // real collision detection, so Sigma's built-in label pass is disabled.
      renderLabels: false,
      labelFont: 'system-ui, -apple-system, sans-serif',
      defaultEdgeColor: '#333',
      defaultNodeColor: '#555',
      defaultEdgeType: 'curved',
      // Sort by the reducer's zIndex so hover-incident edges paint above the
      // dimmed background edges.
      zIndex: true,
      edgeProgramClasses: {
        curved: EdgeCurveProgram,
      },
      minCameraRatio: 0.01,
      maxCameraRatio: 10,
      stagePadding: 40,
      // Hover pill + ring are drawn on our own labelCanvas (drawLabels) so
      // they stack above atom/cluster labels. Sigma's hover canvas sits below
      // labelCanvas, so drawing the hover pill here would render it behind.
      defaultDrawNodeHover: () => {},
      nodeReducer: (node, attrs) => {
        const hovered = hoveredNodeRef.current;
        const pinned = pinnedNodeRef.current;
        if (hovered || pinned) {
          if (hovered && node === hovered) return { ...attrs, zIndex: 2 };
          if (pinned && node === pinned) return { ...attrs, zIndex: 2 };
          const isNeighbor =
            (hovered && neighborsRef.current.get(hovered)?.has(node)) ||
            (pinned && neighborsRef.current.get(pinned)?.has(node));
          if (isNeighbor) return { ...attrs, zIndex: 1 };
          // Non-neighbors dim. Hover fades in/out via hoverAnim; pin holds
          // the dim at full strength so edges/nodes stay faded after the
          // cursor moves off the pinned node.
          const dim = pinned ? 1 : hoverAnimRef.current;
          const rgb = parseRgbColor(attrs.color as string);
          const color = rgb
            ? `rgb(${Math.round(rgb[0] + (60 - rgb[0]) * dim)},${Math.round(rgb[1] + (60 - rgb[1]) * dim)},${Math.round(rgb[2] + (60 - rgb[2]) * dim)})`
            : attrs.color;
          return {
            ...attrs,
            color,
            size: (attrs.size || 4) * (1 - 0.45 * dim),
            label: dim > 0.5 ? '' : (attrs.label as string),
          };
        }
        const tagId = selectedTagRef.current;
        if (!tagId) return attrs;
        const tagIds = (attrs as any).tagIds as string[] | undefined;
        const matches = tagIds?.includes(tagId);
        if (matches) return attrs;
        return {
          ...attrs,
          color: 'rgba(50, 50, 50, 0.3)',
          size: (attrs.size || 4) * 0.6,
          label: '',
        };
      },
      edgeReducer: (edge, attrs) => {
        const w = (attrs as any).weight ?? 0.5;
        const hovered = hoveredNodeRef.current;
        const pinned = pinnedNodeRef.current;
        const t = themeRef.current;
        const anim = edgeAnimProgress.current;
        if (hovered || pinned) {
          const g = graphRef.current!;
          const src = g.source(edge);
          const dst = g.target(edge);
          const touchesHovered = hovered && (src === hovered || dst === hovered);
          const touchesPinned = pinned && (src === pinned || dst === pinned);
          const h = hoverAnimRef.current;
          if (touchesHovered) {
            // Hover boosts brightness + size on its incident edges.
            const bright = w * anim * (1 + 0.4 * h);
            const size = (0.2 + w * 0.7) * anim + ((0.5 + w * 1.2) * anim - (0.2 + w * 0.7) * anim) * h;
            return {
              ...attrs,
              color: edgeColor(t, Math.min(1, bright)),
              size,
              zIndex: 1,
            };
          }
          if (touchesPinned) {
            // Pinned edges stay at normal brightness — they don't pulse like hover.
            return {
              ...attrs,
              color: edgeColor(t, w * anim),
              size: (0.2 + w * 0.7) * anim,
              zIndex: 1,
            };
          }
          // Non-incident: fade. Pin holds the fade at full so edges stay dim
          // after the cursor leaves the pinned node.
          const dim = pinned ? 1 : h;
          return {
            ...attrs,
            color: edgeColor(t, w * anim * (1 - dim)),
            size: (0.2 + w * 0.7) * anim * (1 - dim),
          };
        }
        if (w < edgeThresholdRef.current) {
          return { ...attrs, hidden: true };
        }
        return {
          ...attrs,
          color: edgeColor(t, w * anim),
          size: (0.2 + w * 0.7) * anim,
        };
      },
    });

    sigmaRef.current = sigma;

    // Cluster labels canvas
    const labelCanvas = document.createElement('canvas');
    labelCanvas.style.position = 'absolute';
    labelCanvas.style.inset = '0';
    labelCanvas.style.pointerEvents = 'none';
    labelCanvas.style.zIndex = '10';
    container.appendChild(labelCanvas);


    function drawLabels() {
      const width = container!.clientWidth;
      const height = container!.clientHeight;
      const ratio = window.devicePixelRatio || 1;
      labelCanvas.width = width * ratio;
      labelCanvas.height = height * ratio;
      labelCanvas.style.width = `${width}px`;
      labelCanvas.style.height = `${height}px`;

      const ctx = labelCanvas.getContext('2d');
      if (!ctx) return;
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      ctx.clearRect(0, 0, width, height);

      const t = themeRef.current;

      // Shared collision list — atom labels avoid cluster pills and vice versa.
      const placed: { x: number; y: number; w: number; h: number }[] = [];
      function collides(rect: { x: number; y: number; w: number; h: number }, pad: number) {
        for (const p of placed) {
          if (
            rect.x - pad < p.x + p.w &&
            rect.x + rect.w + pad > p.x &&
            rect.y - pad < p.y + p.h &&
            rect.y + rect.h + pad > p.y
          ) return true;
        }
        return false;
      }

      // === Cluster labels (highest priority — placed first) ===
      // Skip in interactive preview: clusters are computed over the full graph,
      // so showing them on a filtered subset would mislabel sparse regions.
      const clusterFontSize = 13;
      ctx.font = `600 ${clusterFontSize}px system-ui, -apple-system, sans-serif`;
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';

      const sortedClusters = isInteractivePreview
        ? []
        : [...data!.clusters].sort((a, b) => b.atom_count - a.atom_count);
      const maxClusterLabels = Math.max(4, Math.floor((width * height) / 40000));
      const clusterPad = 24;
      let clusterCount = 0;

      for (const cluster of sortedClusters) {
        if (clusterCount >= maxClusterLabels) break;

        // Compute centroid from actual current node positions
        let cx = 0, cy = 0, count = 0;
        for (const atomId of cluster.atom_ids) {
          if (!graph!.hasNode(atomId)) continue;
          cx += graph!.getNodeAttribute(atomId, 'x') as number;
          cy += graph!.getNodeAttribute(atomId, 'y') as number;
          count++;
        }
        if (count === 0) continue;
        cx /= count;
        cy /= count;
        const pos = sigma!.graphToViewport({ x: cx, y: cy });

        const labelY = pos.y - 20;
        const metrics = ctx.measureText(cluster.label);
        const pillW = metrics.width + 16;
        const pillH = clusterFontSize + 8;
        const rect = {
          x: pos.x - pillW / 2,
          y: labelY - pillH / 2,
          w: pillW,
          h: pillH,
        };

        if (collides(rect, clusterPad)) continue;
        placed.push(rect);
        clusterCount++;

        ctx.fillStyle = t.labelBg;
        ctx.beginPath();
        ctx.roundRect(rect.x, rect.y, pillW, pillH, pillH / 2);
        ctx.fill();
        ctx.strokeStyle = t.labelBorder;
        ctx.lineWidth = 1;
        ctx.stroke();

        ctx.fillStyle = t.labelColor;
        ctx.fillText(cluster.label, pos.x, labelY);
      }

      // === Atom labels (collision-checked against everything already placed) ===
      // Interactive preview (briefing mini-canvas) keeps only a couple of
      // labels at rest — top-connected nodes win, the rest stay quiet until
      // the user hovers them. Pin state is unused here because preview mode
      // never pins.
      const atomFontSize = 12;
      ctx.font = `${atomFontSize}px system-ui, -apple-system, sans-serif`;
      ctx.textBaseline = 'middle';

      const tagFilter = selectedTagRef.current;
      // When a node is pinned, restrict labels to the pinned node + its
      // neighbors so the highlighted subgraph is the only thing named.
      const pinnedId = pinnedNodeRef.current;
      const pinnedNeighbors = pinnedId ? neighborsRef.current.get(pinnedId) : null;
      const minRenderedSize = 4;
      const atomLabelPad = isInteractivePreview ? 40 : 20;
      const maxAtomLabels = isInteractivePreview ? 2 : Infinity;

      type Cand = { vx: number; vy: number; rsize: number; label: string };
      const candidates: Cand[] = [];
      graph!.forEachNode((id, attrs) => {
        if (pinnedId && id !== pinnedId && !pinnedNeighbors?.has(id)) return;
        if (tagFilter) {
          const tagIds = (attrs as any).tagIds as string[] | undefined;
          if (!tagIds?.includes(tagFilter)) return;
        }
        const rsize = sigma!.scaleSize(attrs.size as number);
        if (rsize < minRenderedSize) return;
        const pos = sigma!.graphToViewport({ x: attrs.x as number, y: attrs.y as number });
        // Cull off-screen — generous horizontal margin so labels near the edge still render
        if (pos.x < -200 || pos.x > width + 50 || pos.y < -30 || pos.y > height + 30) return;
        const label = (attrs.label as string) || '';
        if (!label) return;
        candidates.push({ vx: pos.x, vy: pos.y, rsize, label });
      });
      // Largest (most-connected) nodes win label slots in dense regions
      candidates.sort((a, b) => b.rsize - a.rsize);

      // Labels render onto the bounded label canvas, so a label whose right
      // edge would land past `width` gets visually cut off. Flip to the left
      // side of the node when there's more room there — common on the small
      // briefing mini-canvas where nodes near the right edge would otherwise
      // lose their titles.
      ctx.fillStyle = t.nodeLabelColor;
      const edgePad = 2;
      let drawnAtomLabels = 0;
      for (const c of candidates) {
        if (drawnAtomLabels >= maxAtomLabels) break;
        const tw = ctx.measureText(c.label).width;
        const ly = c.vy;
        const rightX = c.vx + c.rsize + 4;
        const leftX = c.vx - c.rsize - 4;
        const rightFits = rightX + tw <= width - edgePad;
        const leftFits = leftX - tw >= edgePad;
        const onLeft = !rightFits && leftFits;
        const rect = onLeft
          ? { x: leftX - tw, y: ly - atomFontSize / 2, w: tw, h: atomFontSize }
          : { x: rightX, y: ly - atomFontSize / 2, w: tw, h: atomFontSize };
        if (collides(rect, atomLabelPad)) continue;
        placed.push(rect);
        if (onLeft) {
          ctx.textAlign = 'right';
          ctx.fillText(c.label, leftX, ly);
        } else {
          ctx.textAlign = 'left';
          ctx.fillText(c.label, rightX, ly);
        }
        drawnAtomLabels++;
      }

      // === Pinned-node ring (persists while a popover is open) ===
      if (pinnedId && graph!.hasNode(pinnedId)) {
        const pAttrs = graph!.getNodeAttributes(pinnedId);
        const pPos = sigma!.graphToViewport({ x: pAttrs.x as number, y: pAttrs.y as number });
        const pSize = sigma!.scaleSize(pAttrs.size as number);
        ctx.beginPath();
        ctx.arc(pPos.x, pPos.y, pSize + 3, 0, Math.PI * 2);
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.75)';
        ctx.lineWidth = 2;
        ctx.stroke();

        // Keep the popover anchored to the node as the camera pans/zooms.
        // The popover ignores these updates after the user manually drags it,
        // so this doesn't fight with intentional repositioning.
        const cRect = container!.getBoundingClientRect();
        const aSize = (pAttrs.size as number) || 4;
        const newAnchor = {
          top: cRect.top + pPos.y - aSize,
          left: cRect.left + pPos.x - aSize,
          bottom: cRect.top + pPos.y + aSize,
          width: aSize * 2,
        };
        setPreviewAnchorRect(prev => {
          if (
            prev &&
            Math.abs(prev.top - newAnchor.top) < 0.5 &&
            Math.abs(prev.left - newAnchor.left) < 0.5
          ) return prev;
          return newAnchor;
        });
      }

      // === Hover ring (drawn on canvas; the pill itself is a DOM element
      // so it can spill past the panel edges instead of getting clipped). ===
      // Suppress the pill/ring for the pinned node — its outline already marks it.
      const hoveredId = hoveredNodeRef.current;
      const hAnim = hoverAnimRef.current;
      const pill = hoverPillRef.current;
      if (hoveredId && hoveredId !== pinnedId && hAnim > 0.01 && graph!.hasNode(hoveredId)) {
        const hAttrs = graph!.getNodeAttributes(hoveredId);
        const hPos = sigma!.graphToViewport({ x: hAttrs.x as number, y: hAttrs.y as number });
        const hSize = sigma!.scaleSize(hAttrs.size as number);
        const hLabel = ((hAttrs as any).fullLabel as string) || (hAttrs.label as string) || '';

        ctx.globalAlpha = hAnim;
        ctx.beginPath();
        ctx.arc(hPos.x, hPos.y, hSize + 2, 0, Math.PI * 2);
        ctx.strokeStyle = 'rgba(255, 255, 255, 0.35)';
        ctx.lineWidth = 1.5;
        ctx.stroke();
        ctx.globalAlpha = 1;

        if (pill) {
          if (pill.textContent !== hLabel) pill.textContent = hLabel;
          // Measure the rendered DOM width so we can decide which side has
          // more room. The pill uses the full untruncated title, so it can
          // be wider than half the panel; we still pick whichever side has
          // more room, but the pill freely spills outside if needed.
          pill.style.visibility = 'hidden';
          pill.style.display = 'block';
          pill.style.left = '0px';
          pill.style.top = '0px';
          const pillW = pill.offsetWidth;
          const pillH = pill.offsetHeight;
          const rightRoom = width - (hPos.x + hSize + 4);
          const leftRoom = hPos.x - hSize - 4;
          const onLeft = leftRoom > rightRoom && pillW > rightRoom;
          const px = onLeft ? hPos.x - hSize - 4 - pillW : hPos.x + hSize + 4;
          const py = hPos.y - pillH / 2;
          pill.style.left = `${px}px`;
          pill.style.top = `${py}px`;
          pill.style.opacity = String(hAnim);
          pill.style.visibility = 'visible';
        }
      } else if (pill && pill.style.display !== 'none') {
        pill.style.display = 'none';
      }
    }

    sigma.on('afterRender', drawLabels);
    requestAnimationFrame(drawLabels);

    // Lock the bounding box to the final layout so Sigma doesn't
    // recompute normalization as nodes move from center outward
    let xMin = Infinity, xMax = -Infinity, yMin = Infinity, yMax = -Infinity;
    for (const pos of Object.values(targetPositions)) {
      if (pos.x < xMin) xMin = pos.x;
      if (pos.x > xMax) xMax = pos.x;
      if (pos.y < yMin) yMin = pos.y;
      if (pos.y > yMax) yMax = pos.y;
    }
    sigma.setCustomBBox({ x: [xMin, xMax], y: [yMin, yMax] });

    // Animate nodes outward from center + fade edges in.
    // Preview mode skips the animation and snaps to final state so the thumbnail
    // shows the real layout immediately on mount. The main view also skips the
    // animation when a pendingCamera or pendingFocusAtomId is set — i.e. the
    // user came from clicking the dashboard preview, and we want to land on the
    // chosen framing/atom without the layout-builds-up flourish.
    let cancelledAnim = false;
    const pendingCamera = !isPreview ? useCanvasStore.getState().pendingCamera : null;
    const pendingFocus = !isPreview ? useCanvasStore.getState().pendingFocusAtomId : null;
    if (isPreview || pendingCamera || pendingFocus) {
      for (const [id, target] of Object.entries(targetPositions)) {
        if (!graph.hasNode(id)) continue;
        graph.setNodeAttribute(id, 'x', target.x);
        graph.setNodeAttribute(id, 'y', target.y);
      }
      edgeAnimProgress.current = 1;
      if (pendingCamera) {
        sigma.getCamera().setState(pendingCamera);
        useCanvasStore.getState().setPendingCamera(null);
      }
      sigma.refresh();
    } else {
      const animStart = performance.now();
      function animateTick(now: number) {
        if (cancelledAnim) return;
        const elapsed = now - animStart;

        // Node positions: 0 → target over 2s, cubic ease-out
        const nt = Math.min(1, elapsed / 2000);
        const ne = 1 - (1 - nt) ** 3;
        for (const [id, target] of Object.entries(targetPositions)) {
          if (!graph.hasNode(id)) continue;
          graph.setNodeAttribute(id, 'x', target.x * ne);
          graph.setNodeAttribute(id, 'y', target.y * ne);
        }

        // Edge fade: 0 → 1 over 2.5s, ease-in
        const et = Math.min(1, elapsed / 2500);
        edgeAnimProgress.current = et * et;

        // setNodeAttribute triggers a render but not a reducer re-run —
        // the edgeReducer reads edgeAnimProgress.current, so force a refresh
        // each tick so edge color/size track the animation.
        sigma.refresh();

        if (nt < 1 || et < 1) {
          requestAnimationFrame(animateTick);
        }
      }
      requestAnimationFrame(animateTick);
    }
    const cancelAnim = () => { cancelledAnim = true; };

    // Helper to show atom preview popover at a node's screen position.
    // Pinning is handled by clickNode / focusAtom so this stays purely positional.
    const showAtomPreview = (atomId: string) => {
      if (!graph.hasNode(atomId) || !sigma) return;
      const nodeAttrs = graph.getNodeAttributes(atomId);
      const viewportPos = sigma.graphToViewport({ x: nodeAttrs.x as number, y: nodeAttrs.y as number });
      const containerRect = container!.getBoundingClientRect();
      const nodeSize = (nodeAttrs.size as number) || 4;
      const screenX = containerRect.left + viewportPos.x;
      const screenY = containerRect.top + viewportPos.y;
      setPreviewAnchorRect({
        top: screenY - nodeSize,
        left: screenX - nodeSize,
        bottom: screenY + nodeSize,
        width: nodeSize * 2,
      });
      setPreviewAtomId(atomId);
    };

    // Pin a node so hover emphasis stays on it while its popover is open.
    const pinNode = (atomId: string) => {
      if (!graph.hasNode(atomId)) return;
      pinnedNodeRef.current = atomId;
      hoveredNodeRef.current = atomId;
      hoverTargetRef.current = 1;
      startHoverAnimRef.current();
    };

    // Build a controller both modes use. Main mode registers it in the global
    // `controller` slot (driven by the chat agent). Preview mode registers it
    // in the separate `previewController` slot so dashboard widgets can drive
    // thumbnail focus without touching the chat agent's target.
    const bboxW = xMax - xMin || 1;
    const bboxH = yMax - yMin || 1;
    const graphToCamera = (gx: number, gy: number) => ({
      x: (gx - xMin) / bboxW,
      y: (gy - yMin) / bboxH,
    });

    const controller = {
      zoomToCluster: (clusterLabel: string) => {
        const cluster = data.clusters.find(
          (c) => c.label.toLowerCase() === clusterLabel.toLowerCase()
        );
        if (!cluster || !graph || !sigma) return;
        let cx = 0, cy = 0, count = 0;
        for (const atomId of cluster.atom_ids) {
          if (!graph.hasNode(atomId)) continue;
          cx += graph.getNodeAttribute(atomId, 'x') as number;
          cy += graph.getNodeAttribute(atomId, 'y') as number;
          count++;
        }
        if (count === 0) return;
        cx /= count;
        cy /= count;
        const cam = graphToCamera(cx, cy);
        sigma.getCamera().animate({ x: cam.x, y: cam.y, ratio: 0.3 }, { duration: 800 });
      },
      focusAtom: (atomId: string) => {
        if (!graph.hasNode(atomId) || !sigma) return;
        const gx = graph.getNodeAttribute(atomId, 'x') as number;
        const gy = graph.getNodeAttribute(atomId, 'y') as number;
        const cam = graphToCamera(gx, gy);
        // ratio 0.15 was visually too tight when arriving from the briefing
        // mini-canvas — the atom filled the screen with no surrounding
        // context. 0.35 keeps the focused atom prominent while showing the
        // neighborhood it sits in.
        sigma.getCamera().animate({ x: cam.x, y: cam.y, ratio: 0.35 }, { duration: 600 });
        // Main view shows the popover after the camera settles; preview stays quiet.
        if (!isPreview) setTimeout(() => { pinNode(atomId); showAtomPreview(atomId); }, 650);
      },
      getCameraState: () => {
        if (!sigma) return null;
        const s = sigma.getCamera().getState();
        return { x: s.x, y: s.y, ratio: s.ratio, angle: s.angle };
      },
    };

    // Hover + click handlers run in main mode AND in interactive preview.
    // In interactive preview, clicks bubble up to the parent (which opens the
    // main canvas focused on the chosen atom) instead of opening a popover.
    const isInteractive = !isPreview || isInteractivePreview;
    if (isInteractive) {
      // Hover animation: exponential ease toward target (0 or 1).
      // Loop stops itself when target is reached, so idle cost is zero.
      let hoverRaf: number | null = null;
      const tickHover = () => {
        const diff = hoverTargetRef.current - hoverAnimRef.current;
        if (Math.abs(diff) < 0.005) {
          hoverAnimRef.current = hoverTargetRef.current;
          if (hoverTargetRef.current === 0) hoveredNodeRef.current = null;
          sigma.refresh();
          hoverRaf = null;
          return;
        }
        hoverAnimRef.current += diff * 0.22; // ~10–12 frames to close
        sigma.refresh();
        hoverRaf = requestAnimationFrame(tickHover);
      };
      const startHoverAnim = () => {
        if (hoverRaf !== null) return;
        hoverRaf = requestAnimationFrame(tickHover);
      };
      startHoverAnimRef.current = startHoverAnim;
      sigma.on('clickNode', ({ node }) => {
        if (isInteractivePreview) {
          onPreviewNodeClickRef.current?.(node);
          return;
        }
        // Main view: pin the clicked node so emphasis persists while the popover is open.
        pinNode(node);
        showAtomPreview(node);
      });
      sigma.on('enterNode', ({ node }) => {
        hoveredNodeRef.current = node;
        hoverTargetRef.current = 1;
        startHoverAnim();
      });
      sigma.on('leaveNode', () => {
        hoverTargetRef.current = 0;
        startHoverAnim();
      });
    }

    if (isPreview) {
      useCanvasStore.getState().registerPreviewController(controller);
    } else {
      const { registerController, setCanvasData } = useCanvasStore.getState();
      setCanvasData(data);
      registerController(controller);

      // If a node was clicked in the briefing mini-canvas, the main view
      // mounts here. Land directly on that atom (camera + popover) so the
      // user sees it selected without any post-mount panning.
      const pendingFocusId = useCanvasStore.getState().pendingFocusAtomId;
      if (pendingFocusId && graph.hasNode(pendingFocusId)) {
        useCanvasStore.getState().setPendingFocusAtomId(null);
        controller.focusAtom(pendingFocusId);
      } else if (pendingFocusId) {
        // The atom isn't in the graph (e.g. no embedding yet) — drop the
        // request so a stale focus doesn't fire later.
        useCanvasStore.getState().setPendingFocusAtomId(null);
      }
    }

    return () => {
      cancelAnim();
      const store = useCanvasStore.getState();
      if (isPreview) {
        store.unregisterPreviewController();
      } else {
        store.unregisterController();
      }
      sigma.kill();
      labelCanvas.remove();
      sigmaRef.current = null;
      graphRef.current = null;
    };
  }, [data, isPreview, filterKey]); // intentionally exclude theme — handled below

  // Update colors when theme changes (without recreating graph)
  useEffect(() => {
    const graph = graphRef.current;
    const sigma = sigmaRef.current;
    if (!graph || !sigma || !graphDataRef.current) return;

    const { edgeCounts, maxEdges } = graphDataRef.current;

    // Update node colors
    graph.forEachNode((node, attrs) => {
      const connectivity = (edgeCounts.get(node) || 0) / maxEdges;
      graph.setNodeAttribute(node, 'color', nodeColor(theme, connectivity, (attrs as any).clusterIndex));
    });

    // Atom label color comes from themeRef inside drawLabels — just trigger a refresh.
    // Edges update via edgeReducer (also reads themeRef.current).
    sigma.refresh();
  }, [theme]);

  // Refresh when selected tag changes (nodeReducer reads selectedTagRef)
  useEffect(() => {
    sigmaRef.current?.refresh();
  }, [selectedTagId]);

  // Continuously refresh sigma during chat sidebar transition so the graph
  // resizes smoothly. Expanding the pane collapses this column to zero width
  // without unmounting it, so the same catch-up applies.
  const chatSidebarOpen = useUIStore(s => s.chatSidebarOpen);
  const chatSidebarExpanded = useUIStore(s => s.chatSidebarExpanded);
  useEffect(() => {
    const start = performance.now();
    let raf: number;
    function tick(now: number) {
      sigmaRef.current?.refresh();
      if (now - start < 350) {
        raf = requestAnimationFrame(tick);
      }
    }
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [chatSidebarOpen, chatSidebarExpanded]);

  // Subscribe to canvas action events from the chat agent.
  // Preview instances don't own the controller and shouldn't react to chat actions.
  useEffect(() => {
    if (isPreview) return;
    const transport = getTransport();
    const unsub = transport.subscribe<{ conversation_id: string; action: string; params: Record<string, string> }>(
      'chat-canvas-action',
      (payload) => {
        const ctrl = useCanvasStore.getState().controller;
        if (!ctrl) return;
        if (payload.action === 'zoom_to_cluster') {
          ctrl.zoomToCluster(payload.params.cluster_label);
        } else if (payload.action === 'focus_atom') {
          ctrl.focusAtom(payload.params.atom_id);
        }
      }
    );
    return () => unsub();
  }, [isPreview]);

  // Animate edge threshold changes
  const thresholdAnimRef = useRef<number | null>(null);
  useEffect(() => {
    const sigma = sigmaRef.current;
    if (!sigma) {
      edgeThresholdRef.current = edgeThreshold;
      return;
    }
    const from = edgeThresholdRef.current;
    const to = edgeThreshold;
    if (Math.abs(from - to) < 0.001) return;

    if (thresholdAnimRef.current) cancelAnimationFrame(thresholdAnimRef.current);
    const start = performance.now();
    const duration = 400;
    function tick(now: number) {
      const t = Math.min(1, (now - start) / duration);
      const eased = 1 - (1 - t) ** 2; // ease out quad
      edgeThresholdRef.current = from + (to - from) * eased;
      sigma!.refresh();
      if (t < 1) {
        thresholdAnimRef.current = requestAnimationFrame(tick);
      } else {
        thresholdAnimRef.current = null;
      }
    }
    thresholdAnimRef.current = requestAnimationFrame(tick);
  }, [edgeThreshold]);

  return (
    <div className="flex flex-col h-full w-full relative">
      <div
        className="flex-1 relative overflow-hidden"
        style={{ backgroundColor: isPreview ? 'var(--color-bg-main)' : theme.background }}
      >
        {isLoading && (
          <div className="absolute inset-0 flex items-center justify-center z-10">
            <div className="flex items-center gap-2 text-[var(--color-text-secondary)]">
              <Loader2 className={`animate-spin ${isPreview ? 'h-4 w-4' : 'h-5 w-5'}`} strokeWidth={2} />
              {!isPreview && <span className="text-sm">Computing layout...</span>}
            </div>
          </div>
        )}

        {error && (
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="text-center text-[var(--color-text-secondary)]">
              {isPreview ? (
                <p className="text-xs">Canvas unavailable</p>
              ) : (
                <>
                  <p className="text-lg mb-2">Error loading canvas</p>
                  <p className="text-sm">{error}</p>
                </>
              )}
            </div>
          </div>
        )}

        {!isLoading && data && data.atoms.length === 0 && (
          <div className="absolute inset-0 flex items-center justify-center">
            <div className="text-center text-[var(--color-text-secondary)]">
              {isPreview ? (
                <p className="text-xs">No atoms with embeddings yet</p>
              ) : (
                <>
                  <p className="text-lg mb-2">No atoms with embeddings</p>
                  <p className="text-sm">Create some atoms and wait for embeddings to generate</p>
                </>
              )}
            </div>
          </div>
        )}

        <div
          ref={containerRef}
          className={`w-full h-full ${isPreview && !isInteractivePreview ? 'pointer-events-none' : ''}`}
          style={isPreview ? undefined : { minHeight: 200 }}
        />

        {/* Click-through overlay only for the static thumbnail preview.
            Interactive preview handles its own clicks via clickNode. */}
        {isPreview && !isInteractivePreview && (
          <button
            type="button"
            onClick={onPreviewClick}
            className="absolute inset-0 z-20 cursor-pointer bg-transparent hover:bg-white/[0.03] transition-colors"
            aria-label="Open canvas view"
          />
        )}

        {/* Theme picker + edge slider — main view only */}
        {!isPreview && !isLoading && data && data.atoms.length > 0 && (
          <div className="absolute bottom-4 left-4 z-20 flex flex-col gap-2">
            <div className="flex items-center gap-1.5">
              <button
                onClick={() => setThemePickerOpen(!themePickerOpen)}
                title="Change theme"
                className="w-6 h-6 rounded-full border border-white/20 hover:border-white/40 transition-all flex-shrink-0"
                style={{
                  background: `linear-gradient(135deg, rgb(${theme.nodeMin.join(',')}), rgb(${theme.nodeMax.join(',')}))`,
                }}
              />
              <div
                className={`flex gap-1.5 overflow-hidden transition-all duration-200 ${
                  themePickerOpen ? 'max-w-[200px] opacity-100' : 'max-w-0 opacity-0'
                }`}
              >
                {CANVAS_THEMES.filter(t => t.id !== theme.id).map((t) => (
                  <button
                    key={t.id}
                    onClick={() => { setTheme(t); setThemePickerOpen(false); }}
                    title={t.name}
                    className="w-5 h-5 rounded-full border border-white/15 hover:border-white/40 transition-all flex-shrink-0"
                    style={{
                      background: `linear-gradient(135deg, rgb(${t.nodeMin.join(',')}), rgb(${t.nodeMax.join(',')}))`,
                    }}
                  />
                ))}
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              <input
                type="range"
                min={0}
                max={100}
                value={(1 - edgeThreshold) * 100}
                onChange={(e) => setEdgeThreshold(1 - Number(e.target.value) / 100)}
                className="w-20 h-1 appearance-none bg-white/10 rounded-full cursor-pointer
                  [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-2.5 [&::-webkit-slider-thumb]:h-2.5
                  [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white/60"
                title={`Edges: ${Math.round((1 - edgeThreshold) * 100)}%`}
              />
              <span className="text-[9px] text-white/30">
                {Math.round((1 - edgeThreshold) * 100)}%
              </span>
            </div>
          </div>
        )}

        {/* Atom preview popover — main view only */}
        {!isPreview && previewAtomId && previewAnchorRect && (
          <AtomPreviewPopover
            atomId={previewAtomId}
            anchorRect={previewAnchorRect}
            onClose={closePreview}
            onViewAtom={(atomId, opts) => {
              closePreview();
              openReader(atomId, undefined, opts);
            }}
          />
        )}

      </div>

      {/* Hover pill — lives outside the overflow-hidden clipping div so a
          long title can extend past the panel edges instead of getting cut
          off by the canvas's drawing surface. drawLabels imperatively sets
          its position, content, and opacity each frame. */}
      <div
        ref={hoverPillRef}
        className="absolute pointer-events-none whitespace-nowrap z-30
                   px-1.5 py-1 rounded text-[13px] leading-none
                   bg-[rgba(20,20,20,0.92)] text-[#e8e8e8]
                   border border-white/10"
        style={{ display: 'none' }}
      />
    </div>
  );
}
