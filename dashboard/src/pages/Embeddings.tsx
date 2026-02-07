import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { getStoredRole, pointsApi } from "@/api/ferresdb";
import { useCollections } from "@/hooks/useCollections";
import { useEmbedding } from "@/hooks/useEmbedding";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { Badge } from "@/components/ui/Badge";
import { Skeleton } from "@/components/ui/Skeleton";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/Tabs";
import {
  Cpu,
  Loader2,
  CheckCircle2,
  XCircle,
  Upload,
  Zap,
  Copy,
  ArrowRight,
} from "lucide-react";
import { EMBEDDING_MODELS } from "@/types";
import type { EmbeddingProvider, EmbeddingResult } from "@/types";

export const Embeddings = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === "viewer") navigate("/", { replace: true });
  }, [role, navigate]);

  const [tab, setTab] = useState("single");

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold text-gray-50">Embedding Studio</h1>
        <p className="text-gray-400 mt-2">
          Generate embeddings with OpenAI or Gemini and upsert them into collections
        </p>
      </div>

      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="single">Single Embed</TabsTrigger>
          <TabsTrigger value="batch">Batch Embed</TabsTrigger>
          <TabsTrigger value="pipeline">Pipeline</TabsTrigger>
        </TabsList>

        <TabsContent value="single">
          <SingleEmbedTab />
        </TabsContent>
        <TabsContent value="batch">
          <BatchEmbedTab />
        </TabsContent>
        <TabsContent value="pipeline">
          <PipelineTab />
        </TabsContent>
      </Tabs>
    </div>
  );
};

// ─── Shared: Provider config panel ────────────────────────────────────

function ProviderConfig({
  provider,
  setProvider,
  apiKey,
  setApiKey,
  model,
  setModel,
}: {
  provider: EmbeddingProvider;
  setProvider: (p: EmbeddingProvider) => void;
  apiKey: string;
  setApiKey: (k: string) => void;
  model: string;
  setModel: (m: string) => void;
}) {
  const models = EMBEDDING_MODELS.filter((m) => m.provider === provider);

  return (
    <div className="space-y-4">
      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Provider</label>
        <div className="grid grid-cols-2 gap-2">
          {(["openai", "gemini"] as EmbeddingProvider[]).map((p) => (
            <button
              key={p}
              onClick={() => {
                setProvider(p);
                const defaultModel = EMBEDDING_MODELS.find((m) => m.provider === p);
                if (defaultModel) setModel(defaultModel.id);
              }}
              className={`px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                provider === p
                  ? "bg-orange-500 text-white"
                  : "bg-bg-tertiary text-gray-400 hover:bg-bg-tertiary/80"
              }`}
            >
              {p === "openai" ? "OpenAI" : "Gemini"}
            </button>
          ))}
        </div>
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">Model</label>
        <select
          value={model}
          onChange={(e) => setModel(e.target.value)}
          className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
        >
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name} ({m.dimensions}d)
            </option>
          ))}
        </select>
      </div>

      <div>
        <label className="block text-sm font-medium text-gray-400 mb-2">API Key</label>
        <Input
          type="password"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
          placeholder={`Enter your ${provider === "openai" ? "OpenAI" : "Gemini"} API key`}
          className="bg-bg-secondary border-bg-tertiary text-gray-50"
        />
      </div>
    </div>
  );
}

// ─── Vector Preview ───────────────────────────────────────────────────

function VectorPreview({ result }: { result: EmbeddingResult }) {
  const [copied, setCopied] = useState(false);
  const magnitude = Math.sqrt(result.vector.reduce((s, v) => s + v * v, 0));
  const preview = result.vector.slice(0, 8);

  const handleCopy = () => {
    navigator.clipboard.writeText(JSON.stringify(result.vector));
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="bg-bg-tertiary rounded-lg p-4 border border-bg-tertiary space-y-3">
      <div className="flex items-center justify-between">
        <h4 className="text-sm font-semibold text-gray-50">Vector Preview</h4>
        <button
          onClick={handleCopy}
          className="text-xs text-gray-400 hover:text-gray-200 flex items-center gap-1"
        >
          <Copy className="h-3 w-3" />
          {copied ? "Copied!" : "Copy"}
        </button>
      </div>

      <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
        <div>
          <p className="text-xs text-gray-400">Dimensions</p>
          <p className="text-sm font-semibold text-gray-50">{result.dimensions}</p>
        </div>
        <div>
          <p className="text-xs text-gray-400">Magnitude</p>
          <p className="text-sm font-semibold text-gray-50">{magnitude.toFixed(4)}</p>
        </div>
        <div>
          <p className="text-xs text-gray-400">Model</p>
          <p className="text-sm font-semibold text-gray-50">{result.model}</p>
        </div>
        <div>
          <p className="text-xs text-gray-400">Time</p>
          <p className="text-sm font-semibold text-orange-500">{result.took_ms} ms</p>
        </div>
      </div>

      <div className="text-xs font-mono text-gray-400 bg-bg-secondary rounded p-2 overflow-x-auto">
        [{preview.map((v) => v.toFixed(6)).join(", ")}
        {result.vector.length > 8 ? `, ... (${result.vector.length - 8} more)` : ""}]
      </div>
    </div>
  );
}

// ─── Tab 1: Single Embed ──────────────────────────────────────────────

function SingleEmbedTab() {
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  const { embed, isLoading, error: embedError, clearError } = useEmbedding();

  const [provider, setProvider] = useState<EmbeddingProvider>("openai");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("text-embedding-3-small");
  const [text, setText] = useState("");
  const [result, setResult] = useState<EmbeddingResult | null>(null);

  // Upsert
  const [selectedCollection, setSelectedCollection] = useState("");
  const [pointId, setPointId] = useState("");
  const [metadataJson, setMetadataJson] = useState('{"text": ""}');
  const [upsertStatus, setUpsertStatus] = useState<"idle" | "loading" | "success" | "error">(
    "idle",
  );
  const [upsertError, setUpsertError] = useState<string | null>(null);

  const handleEmbed = async () => {
    if (!text || !apiKey) return;
    clearError();
    try {
      const res = await embed(text, provider, apiKey, model);
      setResult(res);
      // Auto-fill metadata text
      setMetadataJson(JSON.stringify({ text }, null, 2));
    } catch {
      // error handled by hook
    }
  };

  const handleUpsert = async () => {
    if (!result || !selectedCollection || !pointId) return;
    setUpsertStatus("loading");
    setUpsertError(null);
    try {
      let metadata: Record<string, unknown> = {};
      try {
        metadata = JSON.parse(metadataJson);
      } catch {
        throw new Error("Invalid metadata JSON");
      }
      await pointsApi.upsert(selectedCollection, [
        { id: pointId, vector: result.vector, metadata },
      ]);
      setUpsertStatus("success");
    } catch (err) {
      setUpsertError(err instanceof Error ? err.message : "Upsert failed");
      setUpsertStatus("error");
    }
  };

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6 mt-4">
      {/* Left: Config + Input */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Cpu className="h-5 w-5" />
            Generate Embedding
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <ProviderConfig
            provider={provider}
            setProvider={setProvider}
            apiKey={apiKey}
            setApiKey={setApiKey}
            model={model}
            setModel={setModel}
          />

          <div>
            <label className="block text-sm font-medium text-gray-400 mb-2">Text</label>
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="Enter text to embed..."
              rows={5}
              className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-orange-500"
            />
          </div>

          <Button
            onClick={handleEmbed}
            disabled={isLoading || !text || !apiKey}
            className="w-full"
            variant="primary"
          >
            {isLoading ? (
              <>
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                Embedding...
              </>
            ) : (
              <>
                <Cpu className="h-4 w-4 mr-2" />
                Generate Embedding
              </>
            )}
          </Button>

          {embedError && (
            <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
              <XCircle className="h-4 w-4 text-red-500 shrink-0" />
              <p className="text-sm text-red-400">{embedError}</p>
            </div>
          )}
        </CardContent>
      </Card>

      {/* Right: Result + Upsert */}
      <Card>
        <CardHeader>
          <CardTitle>Result</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {result ? (
            <>
              <VectorPreview result={result} />

              <div className="border-t border-bg-tertiary pt-4 space-y-4">
                <h4 className="text-sm font-semibold text-gray-50 flex items-center gap-2">
                  <Upload className="h-4 w-4" />
                  Upsert to Collection
                </h4>

                <div>
                  <label className="block text-sm font-medium text-gray-400 mb-2">Collection</label>
                  {collectionsLoading ? (
                    <Skeleton className="h-10 w-full" />
                  ) : (
                    <select
                      value={selectedCollection}
                      onChange={(e) => setSelectedCollection(e.target.value)}
                      className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
                    >
                      <option value="">Select a collection</option>
                      {Array.isArray(collections) &&
                        collections.map((c) => (
                          <option key={c.name} value={c.name}>
                            {c.name} ({c.dimension}d, {c.num_points ?? c.point_count ?? 0} pts)
                          </option>
                        ))}
                    </select>
                  )}
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-400 mb-2">Point ID</label>
                  <Input
                    value={pointId}
                    onChange={(e) => setPointId(e.target.value)}
                    placeholder="unique-point-id"
                    className="bg-bg-secondary border-bg-tertiary text-gray-50"
                  />
                </div>

                <div>
                  <label className="block text-sm font-medium text-gray-400 mb-2">
                    Metadata (JSON)
                  </label>
                  <textarea
                    value={metadataJson}
                    onChange={(e) => setMetadataJson(e.target.value)}
                    rows={4}
                    className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 font-mono focus:outline-none focus:ring-2 focus:ring-orange-500"
                  />
                </div>

                <Button
                  onClick={handleUpsert}
                  disabled={upsertStatus === "loading" || !selectedCollection || !pointId}
                  className="w-full"
                  variant="primary"
                >
                  {upsertStatus === "loading" ? (
                    <>
                      <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                      Upserting...
                    </>
                  ) : (
                    <>
                      <Upload className="h-4 w-4 mr-2" />
                      Upsert Point
                    </>
                  )}
                </Button>

                {upsertStatus === "success" && (
                  <div className="flex items-center gap-2 p-3 rounded-md bg-green-500/10 border border-green-500/50">
                    <CheckCircle2 className="h-4 w-4 text-green-500" />
                    <p className="text-sm text-green-400">Point upserted successfully!</p>
                  </div>
                )}

                {upsertError && (
                  <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                    <XCircle className="h-4 w-4 text-red-500 shrink-0" />
                    <p className="text-sm text-red-400">{upsertError}</p>
                  </div>
                )}
              </div>
            </>
          ) : (
            <div className="text-center py-16">
              <Cpu className="h-12 w-12 mx-auto text-gray-600 mb-4" />
              <p className="text-gray-400">Generate an embedding to see the result here.</p>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

// ─── Tab 2: Batch Embed ───────────────────────────────────────────────

type BatchItem = {
  id: string;
  text: string;
  status: "pending" | "done" | "error";
  vector?: number[];
  error?: string;
};

function BatchEmbedTab() {
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  const { embedBatch, isLoading, error: embedError, clearError } = useEmbedding();

  const [provider, setProvider] = useState<EmbeddingProvider>("openai");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("text-embedding-3-small");
  const [rawInput, setRawInput] = useState("");
  const [items, setItems] = useState<BatchItem[]>([]);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  // Upsert
  const [selectedCollection, setSelectedCollection] = useState("");
  const [upsertStatus, setUpsertStatus] = useState<"idle" | "loading" | "success" | "error">(
    "idle",
  );
  const [upsertError, setUpsertError] = useState<string | null>(null);

  const parseInput = () => {
    const lines = rawInput
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.length > 0);

    // Try JSON array first
    try {
      const parsed = JSON.parse(rawInput);
      if (Array.isArray(parsed)) {
        return parsed.map((item, i) => ({
          id: item.id || `batch-${i + 1}`,
          text: item.text || String(item),
          status: "pending" as const,
        }));
      }
    } catch {
      // Not JSON — use line-by-line
    }

    return lines.map((line, i) => ({
      id: `batch-${i + 1}`,
      text: line,
      status: "pending" as const,
    }));
  };

  const handleEmbed = async () => {
    if (!rawInput || !apiKey) return;
    clearError();

    const parsed = parseInput();
    setItems(parsed);
    setProgress({ done: 0, total: parsed.length });

    try {
      const texts = parsed.map((p) => p.text);
      const results = await embedBatch(texts, provider, apiKey, model, (done, total) => {
        setProgress({ done, total });
      });

      setItems(
        parsed.map((item, i) => ({
          ...item,
          status: "done" as const,
          vector: results[i]?.vector,
        })),
      );
      setProgress({ done: parsed.length, total: parsed.length });
    } catch {
      // Mark remaining as error
      setItems((prev) =>
        prev.map((item) =>
          item.status === "pending" ? { ...item, status: "error", error: "Batch failed" } : item,
        ),
      );
    }
  };

  const handleUpsertAll = async () => {
    const ready = items.filter((i) => i.status === "done" && i.vector);
    if (ready.length === 0 || !selectedCollection) return;
    setUpsertStatus("loading");
    setUpsertError(null);

    try {
      const points = ready.map((item) => ({
        id: item.id,
        vector: item.vector!,
        metadata: { text: item.text },
      }));
      await pointsApi.upsert(selectedCollection, points);
      setUpsertStatus("success");
    } catch (err) {
      setUpsertError(err instanceof Error ? err.message : "Upsert failed");
      setUpsertStatus("error");
    }
  };

  const readyCount = items.filter((i) => i.status === "done" && i.vector).length;

  return (
    <div className="space-y-6 mt-4">
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Config */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Upload className="h-5 w-5" />
              Batch Configuration
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <ProviderConfig
              provider={provider}
              setProvider={setProvider}
              apiKey={apiKey}
              setApiKey={setApiKey}
              model={model}
              setModel={setModel}
            />

            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Texts (one per line or JSON array)
              </label>
              <textarea
                value={rawInput}
                onChange={(e) => setRawInput(e.target.value)}
                placeholder={'Line-by-line:\nHello world\nAnother sentence\n\nOr JSON:\n[{"id": "doc-1", "text": "Hello"}]'}
                rows={8}
                className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 placeholder-gray-500 font-mono focus:outline-none focus:ring-2 focus:ring-orange-500"
              />
            </div>

            <Button
              onClick={handleEmbed}
              disabled={isLoading || !rawInput || !apiKey}
              className="w-full"
              variant="primary"
            >
              {isLoading ? (
                <>
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  Embedding {progress ? `${progress.done}/${progress.total}` : "..."}
                </>
              ) : (
                <>
                  <Cpu className="h-4 w-4 mr-2" />
                  Embed All
                </>
              )}
            </Button>

            {embedError && (
              <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                <XCircle className="h-4 w-4 text-red-500 shrink-0" />
                <p className="text-sm text-red-400">{embedError}</p>
              </div>
            )}
          </CardContent>
        </Card>

        {/* Results */}
        <Card>
          <CardHeader>
            <CardTitle>
              Results {items.length > 0 && `(${readyCount}/${items.length})`}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {progress && (
              <div className="space-y-1">
                <div className="flex justify-between text-xs text-gray-400">
                  <span>Progress</span>
                  <span>
                    {progress.done}/{progress.total}
                  </span>
                </div>
                <div className="w-full bg-bg-secondary rounded-full h-2">
                  <div
                    className="bg-orange-500 h-2 rounded-full transition-all"
                    style={{ width: `${(progress.done / progress.total) * 100}%` }}
                  />
                </div>
              </div>
            )}

            {items.length > 0 ? (
              <div className="max-h-[400px] overflow-y-auto space-y-2">
                {items.map((item) => (
                  <div
                    key={item.id}
                    className="flex items-center justify-between bg-bg-tertiary rounded-lg px-3 py-2"
                  >
                    <div className="flex-1 min-w-0">
                      <p className="text-xs font-mono text-gray-400">{item.id}</p>
                      <p className="text-sm text-gray-200 truncate">{item.text}</p>
                    </div>
                    <Badge
                      variant={
                        item.status === "done"
                          ? "success"
                          : item.status === "error"
                            ? "danger"
                            : "default"
                      }
                      className="ml-2 shrink-0"
                    >
                      {item.status === "done"
                        ? `${item.vector?.length}d`
                        : item.status === "error"
                          ? "Error"
                          : "Pending"}
                    </Badge>
                  </div>
                ))}
              </div>
            ) : (
              <div className="text-center py-12">
                <p className="text-gray-400">Enter texts and click "Embed All" to batch process.</p>
              </div>
            )}

            {readyCount > 0 && (
              <div className="border-t border-bg-tertiary pt-4 space-y-3">
                <h4 className="text-sm font-semibold text-gray-50 flex items-center gap-2">
                  <Upload className="h-4 w-4" />
                  Upsert {readyCount} Points
                </h4>
                <div>
                  <label className="block text-sm font-medium text-gray-400 mb-2">Collection</label>
                  {collectionsLoading ? (
                    <Skeleton className="h-10 w-full" />
                  ) : (
                    <select
                      value={selectedCollection}
                      onChange={(e) => setSelectedCollection(e.target.value)}
                      className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
                    >
                      <option value="">Select a collection</option>
                      {Array.isArray(collections) &&
                        collections.map((c) => (
                          <option key={c.name} value={c.name}>
                            {c.name} ({c.dimension}d)
                          </option>
                        ))}
                    </select>
                  )}
                </div>
                <Button
                  onClick={handleUpsertAll}
                  disabled={upsertStatus === "loading" || !selectedCollection}
                  className="w-full"
                  variant="primary"
                >
                  {upsertStatus === "loading" ? (
                    <>
                      <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                      Upserting...
                    </>
                  ) : (
                    <>
                      <Upload className="h-4 w-4 mr-2" />
                      Upsert All ({readyCount} points)
                    </>
                  )}
                </Button>
                {upsertStatus === "success" && (
                  <div className="flex items-center gap-2 p-3 rounded-md bg-green-500/10 border border-green-500/50">
                    <CheckCircle2 className="h-4 w-4 text-green-500" />
                    <p className="text-sm text-green-400">
                      {readyCount} points upserted successfully!
                    </p>
                  </div>
                )}
                {upsertError && (
                  <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                    <XCircle className="h-4 w-4 text-red-500 shrink-0" />
                    <p className="text-sm text-red-400">{upsertError}</p>
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

// ─── Tab 3: Pipeline (Embed + Upsert in one step) ────────────────────

type PipelineItem = {
  id: string;
  text: string;
  metadata: Record<string, unknown>;
  status: "pending" | "embedding" | "upserting" | "done" | "error";
  error?: string;
};

function PipelineTab() {
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  const { embed } = useEmbedding();

  const [provider, setProvider] = useState<EmbeddingProvider>("openai");
  const [apiKey, setApiKey] = useState("");
  const [model, setModel] = useState("text-embedding-3-small");
  const [selectedCollection, setSelectedCollection] = useState("");
  const [rawInput, setRawInput] = useState("");
  const [items, setItems] = useState<PipelineItem[]>([]);
  const [running, setRunning] = useState(false);

  const handleRun = async () => {
    if (!rawInput || !apiKey || !selectedCollection) return;

    const lines = rawInput
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l.length > 0);

    const pipeline: PipelineItem[] = lines.map((text, i) => ({
      id: `pipe-${Date.now()}-${i + 1}`,
      text,
      metadata: { text },
      status: "pending",
    }));

    setItems(pipeline);
    setRunning(true);

    for (let i = 0; i < pipeline.length; i++) {
      const item = pipeline[i];

      // Update status: embedding
      setItems((prev) =>
        prev.map((p, idx) => (idx === i ? { ...p, status: "embedding" } : p)),
      );

      try {
        const result = await embed(item.text, provider, apiKey, model);

        // Update status: upserting
        setItems((prev) =>
          prev.map((p, idx) => (idx === i ? { ...p, status: "upserting" } : p)),
        );

        await pointsApi.upsert(selectedCollection, [
          { id: item.id, vector: result.vector, metadata: item.metadata },
        ]);

        // Update status: done
        setItems((prev) => prev.map((p, idx) => (idx === i ? { ...p, status: "done" } : p)));
      } catch (err) {
        setItems((prev) =>
          prev.map((p, idx) =>
            idx === i
              ? { ...p, status: "error", error: err instanceof Error ? err.message : "Failed" }
              : p,
          ),
        );
      }
    }

    setRunning(false);
  };

  const doneCount = items.filter((i) => i.status === "done").length;
  const errorCount = items.filter((i) => i.status === "error").length;

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6 mt-4">
      {/* Config */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Zap className="h-5 w-5" />
            Text-to-Vector Pipeline
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <ProviderConfig
            provider={provider}
            setProvider={setProvider}
            apiKey={apiKey}
            setApiKey={setApiKey}
            model={model}
            setModel={setModel}
          />

          <div>
            <label className="block text-sm font-medium text-gray-400 mb-2">
              Target Collection
            </label>
            {collectionsLoading ? (
              <Skeleton className="h-10 w-full" />
            ) : (
              <select
                value={selectedCollection}
                onChange={(e) => setSelectedCollection(e.target.value)}
                className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
              >
                <option value="">Select a collection</option>
                {Array.isArray(collections) &&
                  collections.map((c) => (
                    <option key={c.name} value={c.name}>
                      {c.name} ({c.dimension}d, {c.num_points ?? c.point_count ?? 0} pts)
                    </option>
                  ))}
              </select>
            )}
          </div>

          <div>
            <label className="block text-sm font-medium text-gray-400 mb-2">
              Texts (one per line)
            </label>
            <textarea
              value={rawInput}
              onChange={(e) => setRawInput(e.target.value)}
              placeholder="Enter one text per line. Each will be embedded and upserted."
              rows={8}
              className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-orange-500"
            />
          </div>

          <Button
            onClick={handleRun}
            disabled={running || !rawInput || !apiKey || !selectedCollection}
            className="w-full"
            variant="primary"
          >
            {running ? (
              <>
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                Processing {doneCount}/{items.length}...
              </>
            ) : (
              <>
                <Zap className="h-4 w-4 mr-2" />
                Run Pipeline
              </>
            )}
          </Button>
        </CardContent>
      </Card>

      {/* Status */}
      <Card>
        <CardHeader>
          <CardTitle>
            Pipeline Status{" "}
            {items.length > 0 && (
              <span className="text-sm font-normal text-gray-400">
                {doneCount} done, {errorCount} errors, {items.length} total
              </span>
            )}
          </CardTitle>
        </CardHeader>
        <CardContent>
          {items.length > 0 ? (
            <div className="space-y-2 max-h-[500px] overflow-y-auto">
              {items.map((item) => (
                <div
                  key={item.id}
                  className="flex items-center gap-3 bg-bg-tertiary rounded-lg px-3 py-2"
                >
                  <div className="shrink-0">
                    {item.status === "done" && (
                      <CheckCircle2 className="h-4 w-4 text-green-500" />
                    )}
                    {item.status === "error" && <XCircle className="h-4 w-4 text-red-500" />}
                    {(item.status === "embedding" || item.status === "upserting") && (
                      <Loader2 className="h-4 w-4 text-orange-500 animate-spin" />
                    )}
                    {item.status === "pending" && (
                      <div className="h-4 w-4 rounded-full border-2 border-gray-600" />
                    )}
                  </div>
                  <div className="flex-1 min-w-0">
                    <p className="text-sm text-gray-200 truncate">{item.text}</p>
                    {item.error && <p className="text-xs text-red-400">{item.error}</p>}
                  </div>
                  <Badge
                    variant={
                      item.status === "done"
                        ? "success"
                        : item.status === "error"
                          ? "danger"
                          : "default"
                    }
                    className="shrink-0"
                  >
                    {item.status === "embedding"
                      ? "Embedding"
                      : item.status === "upserting"
                        ? "Upserting"
                        : item.status === "done"
                          ? "Done"
                          : item.status === "error"
                            ? "Error"
                            : "Pending"}
                  </Badge>
                </div>
              ))}
            </div>
          ) : (
            <div className="text-center py-16">
              <ArrowRight className="h-12 w-12 mx-auto text-gray-600 mb-4" />
              <p className="text-gray-400">
                Configure the pipeline and click "Run Pipeline" to embed and upsert texts in one
                step.
              </p>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
