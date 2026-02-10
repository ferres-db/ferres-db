import { useParams, useNavigate } from 'react-router-dom';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import ForceGraph2D from 'react-force-graph-2d';
import { useCollection } from '@/hooks/useCollections';
import { graphApi } from '@/api/ferresdb';
import type { GraphNode, GraphEdge, SubgraphResponse } from '@/types';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Skeleton } from '@/components/ui/Skeleton';
import { ArrowLeft, Loader2, Network, PanelRightClose, PanelRightOpen } from 'lucide-react';

/** Node with optional category for coloring (from metadata). */
type GraphNodeWithCategory = GraphNode & {
  category?: string;
  score?: number;
};

const CATEGORY_COLORS: Record<string, string> = {
  default: '#94a3b8',
  category_a: '#22c55e',
  category_b: '#3b82f6',
  category_c: '#a855f7',
  category_d: '#f59e0b',
  category_e: '#ef4444',
};

function categoryFromMetadata(metadata?: Record<string, unknown>): string {
  if (!metadata) return 'default';
  const cat = (metadata.category ?? metadata.type ?? metadata.label ?? metadata.kind) as string | undefined;
  if (typeof cat === 'string') {
    const key = `category_${cat.slice(0, 1).toLowerCase()}`;
    return key in CATEGORY_COLORS ? key : 'default';
  }
  return 'default';
}

function getCategoryColor(category: string): string {
  return CATEGORY_COLORS[category] ?? CATEGORY_COLORS.default;
}

export const GraphExplorer = () => {
  const { name } = useParams<{ name: string }>();
  const navigate = useNavigate();
  const containerRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(800);
  const [height, setHeight] = useState(600);
  const [nodes, setNodes] = useState<GraphNodeWithCategory[]>([]);
  const [links, setLinks] = useState<GraphEdge[]>([]);
  const [selectedNode, setSelectedNode] = useState<GraphNodeWithCategory | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [loadingExpand, setLoadingExpand] = useState<string | null>(null);

  const { isLoading: collectionLoading } = useCollection(name || '');

  const { data: initialSubgraph, isLoading: initialLoading } = useQuery({
    queryKey: ['graph-subgraph', name],
    queryFn: () => graphApi.getSubgraph(name!, { limit: 300 }),
    enabled: !!name,
  });

  useEffect(() => {
    if (!initialSubgraph) return;
    const withCategory: GraphNodeWithCategory[] = initialSubgraph.nodes.map((n) => ({
      ...n,
      category: categoryFromMetadata(n.metadata),
    }));
    setNodes(withCategory);
    setLinks(initialSubgraph.edges ?? []);
  }, [initialSubgraph]);

  const mergeSubgraph = useCallback((resp: SubgraphResponse) => {
    const newNodes = resp.nodes.map((n) => ({
      ...n,
      category: categoryFromMetadata(n.metadata),
    }));
    setNodes((prev) => {
      const byId = new Map(prev.map((n) => [n.id, n]));
      newNodes.forEach((n) => byId.set(n.id, n));
      return Array.from(byId.values());
    });
    setLinks((prev) => {
      const edges = resp.edges ?? [];
      const seen = new Set(prev.map((l) => `${l.source}-${l.target}`));
      const next = [...prev];
      edges.forEach((l) => {
        const key = `${l.source}-${l.target}`;
        const rev = `${l.target}-${l.source}`;
        if (!seen.has(key) && !seen.has(rev)) {
          seen.add(key);
          next.push(l);
        }
      });
      return next;
    });
  }, []);

  const handleNodeClick = useCallback(
    (node: { id?: string }) => {
      const id = node.id ?? (node as GraphNodeWithCategory).id;
      if (!id || !name) return;
      const current = nodes.find((n) => n.id === id);
      if (current) setSelectedNode(current);

      setLoadingExpand(id);
      graphApi
        .getSubgraph(name, { seed: id })
        .then(mergeSubgraph)
        .finally(() => setLoadingExpand(null));
    },
    [name, nodes, mergeSubgraph]
  );

  const graphData = useMemo(
    () => ({
      nodes: nodes.map((n) => ({ ...n })),
      links: links.map((l) => ({ source: l.source, target: l.target })),
    }),
    [nodes, links]
  );

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const resize = () => {
      setWidth(el.offsetWidth);
      setHeight(Math.max(400, el.offsetHeight));
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  if (!name) {
    return (
      <div className="space-y-6">
        <Card>
          <CardContent className="pt-6">
            <p className="text-gray-400">Collection name is required</p>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-4">
          <Button variant="ghost" size="sm" onClick={() => navigate(`/collections/${name}/points`)}>
            <ArrowLeft className="h-4 w-4 mr-2" />
            Back
          </Button>
          <div>
            <h1 className="text-2xl font-semibold tracking-tight text-gray-50 flex items-center gap-2">
              <Network className="h-6 w-6 text-orange-500" />
              Graph Explorer — {name}
            </h1>
            <p className="mt-1 text-sm text-gray-400">
              Click a node to load its neighbors and expand the graph. Sidebar shows node details.
            </p>
          </div>
        </div>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => setSidebarOpen((o) => !o)}
          className="gap-2"
        >
          {sidebarOpen ? <PanelRightClose className="h-4 w-4" /> : <PanelRightOpen className="h-4 w-4" />}
          {sidebarOpen ? 'Hide' : 'Show'} details
        </Button>
      </div>

      <div className="flex gap-4" style={{ minHeight: 500 }}>
        <Card className="flex-1 overflow-hidden flex flex-col min-h-[500px]">
          <CardHeader className="py-3">
            <CardTitle className="text-base flex items-center gap-2">
              {collectionLoading || initialLoading ? (
                <Skeleton className="h-5 w-32" />
              ) : (
                <>
                  {nodes.length} nodes · {links.length} edges
                  {loadingExpand && (
                    <span className="text-sm font-normal text-gray-400 flex items-center gap-1">
                      <Loader2 className="h-4 w-4 animate-spin" />
                      Loading neighbors…
                    </span>
                  )}
                </>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="flex-1 p-0 relative min-h-[400px]" ref={containerRef}>
            {initialLoading ? (
              <div className="absolute inset-0 flex items-center justify-center bg-gray-900/50">
                <Loader2 className="h-8 w-8 animate-spin text-orange-500" />
              </div>
            ) : (
              <ForceGraph2D
                width={width}
                height={height}
                graphData={graphData}
                nodeId="id"
                nodeLabel={(n) => (n as GraphNodeWithCategory).id ?? ''}
                nodeColor={(n) => getCategoryColor((n as GraphNodeWithCategory).category ?? 'default')}
                nodeVal={() => 6}
                linkColor="#475569"
                linkWidth={1}
                onNodeClick={handleNodeClick}
                backgroundColor="rgb(15 23 42)"
              />
            )}
          </CardContent>
        </Card>

        {sidebarOpen && (
          <Card className="w-96 flex-shrink-0 flex flex-col max-h-[600px]">
            <CardHeader className="py-3">
              <CardTitle className="text-base">Node details</CardTitle>
            </CardHeader>
            <CardContent className="flex-1 overflow-auto">
              {selectedNode ? (
                <pre className="text-xs text-gray-300 bg-gray-900/80 p-4 rounded-lg overflow-auto whitespace-pre-wrap break-words">
                  {JSON.stringify(
                    {
                      id: selectedNode.id,
                      metadata: selectedNode.metadata,
                      namespace: selectedNode.namespace,
                      created_at: selectedNode.created_at,
                      relations: selectedNode.relations,
                    },
                    null,
                    2
                  )}
                </pre>
              ) : (
                <p className="text-sm text-gray-500">Click a node to see its details (JSON).</p>
              )}
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
};
