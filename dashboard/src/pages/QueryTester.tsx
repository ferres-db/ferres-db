import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole, pointsApi } from '@/api/ferresdb';
import { useCollections } from '@/hooks/useCollections';
import { useEmbedding } from '@/hooks/useEmbedding';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/Tabs';
import {
  Sparkles,
  Loader2,
  CheckCircle2,
  XCircle,
  Bot,
  Clock,
  Search,
  FileSearch,
  Gauge,
} from 'lucide-react';
import { EMBEDDING_MODELS } from '@/types';
import type {
  SearchResult,
  EmbeddingProvider,
  SearchExplainResponse,
  SearchEstimateResponse,
} from '@/types';

type LlmProvider = 'openai' | 'anthropic' | 'gemini';

export const QueryTester = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === 'viewer') navigate('/', { replace: true });
  }, [role, navigate]);
  return <QueryTesterContent />;
};

const LLM_MODELS: Record<LlmProvider, string[]> = {
  openai: ['gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo', 'gpt-4', 'gpt-3.5-turbo'],
  anthropic: [
    'claude-3-5-sonnet-20241022',
    'claude-3-5-haiku-20241022',
    'claude-3-opus-20240229',
    'claude-3-sonnet-20240229',
  ],
  gemini: ['gemini-1.5-pro', 'gemini-1.5-flash', 'gemini-pro'],
};

const DEFAULT_LLM_MODELS: Record<LlmProvider, string> = {
  openai: 'gpt-4o-mini',
  anthropic: 'claude-3-5-haiku-20241022',
  gemini: 'gemini-1.5-flash',
};

// ─── LLM call helpers ─────────────────────────────────────────────────

async function callOpenAI(prompt: string, apiKey: string, model: string): Promise<string> {
  const response = await fetch('https://api.openai.com/v1/chat/completions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${apiKey}` },
    body: JSON.stringify({ model, messages: [{ role: 'user', content: prompt }], temperature: 0.7 }),
  });
  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || 'Failed to get response from OpenAI');
  }
  const data = await response.json();
  return data.choices[0].message.content || '';
}

async function callAnthropic(prompt: string, apiKey: string, model: string): Promise<string> {
  const response = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'x-api-key': apiKey,
      'anthropic-version': '2023-06-01',
    },
    body: JSON.stringify({ model, max_tokens: 1024, messages: [{ role: 'user', content: prompt }] }),
  });
  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || 'Failed to get response from Anthropic');
  }
  const data = await response.json();
  return data.content[0].text || '';
}

async function callGemini(prompt: string, apiKey: string, model: string): Promise<string> {
  const response = await fetch(
    `https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent?key=${apiKey}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ contents: [{ parts: [{ text: prompt }] }] }),
    },
  );
  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || 'Failed to get response from Gemini');
  }
  const data = await response.json();
  return data.candidates[0].content.parts[0].text || '';
}

function buildRAGPrompt(question: string, contextChunks: SearchResult[]): string {
  const contextParts = contextChunks.map((result, index) => {
    const meta = result.metadata || {};
    const text =
      (meta.text || meta.content || meta.body) as string || '(conteúdo não disponível no metadata)';
    const source = meta.source as string || 'N/A';
    return `[${index + 1}] (fonte: ${source})\n${text}`;
  });
  const context =
    contextParts.length > 0 ? contextParts.join('\n\n---\n\n') : '(Nenhum contexto recuperado)';
  return `Contexto da documentação:\n\n${context}\n\n---\n\nPergunta: ${question}\n\nResponda com base apenas no contexto acima. Se não souber, diga que não encontrou informação.`;
}

// ─── Main Content ─────────────────────────────────────────────────────

function QueryTesterContent() {
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  const { embed } = useEmbedding();

  const [mainTab, setMainTab] = useState('rag');

  // Shared state
  const [llmProvider, setLlmProvider] = useState<LlmProvider>('openai');
  const [llmModel, setLlmModel] = useState<string>(DEFAULT_LLM_MODELS.openai);
  const [embeddingProvider, setEmbeddingProvider] = useState<EmbeddingProvider>('openai');
  const [embeddingModel, setEmbeddingModel] = useState('text-embedding-3-small');
  const [apiKey, setApiKey] = useState('');
  const [selectedCollection, setSelectedCollection] = useState('');
  const [query, setQuery] = useState('');
  const [limit, setLimit] = useState(5);

  // RAG state
  const [isLoading, setIsLoading] = useState(false);
  const [results, setResults] = useState<SearchResult[]>([]);
  const [aiResponse, setAiResponse] = useState<string>('');
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const [timings, setTimings] = useState<{
    embeddingMs: number;
    searchMs: number;
    llmMs: number;
    totalMs: number;
  } | null>(null);

  // Hybrid search
  const [hybridAlpha, setHybridAlpha] = useState('0.5');
  const [hybridTextQuery, setHybridTextQuery] = useState('');
  const [hybridFusion, setHybridFusion] = useState<'weighted' | 'rrf'>('weighted');
  const [hybridRrfK, setHybridRrfK] = useState('60');
  const [hybridResults, setHybridResults] = useState<SearchResult[]>([]);
  const [hybridLoading, setHybridLoading] = useState(false);
  const [hybridError, setHybridError] = useState<string | null>(null);

  // Explain
  const [explainResult, setExplainResult] = useState<SearchExplainResponse | null>(null);
  const [explainLoading, setExplainLoading] = useState(false);
  const [explainError, setExplainError] = useState<string | null>(null);

  // Estimate
  const [estimateResult, setEstimateResult] = useState<SearchEstimateResponse | null>(null);
  const [estimateLoading, setEstimateLoading] = useState(false);
  const [estimateError, setEstimateError] = useState<string | null>(null);

  const handleLlmProviderChange = (provider: LlmProvider) => {
    setLlmProvider(provider);
    setLlmModel(DEFAULT_LLM_MODELS[provider]);
  };

  const handleEmbeddingProviderChange = (provider: EmbeddingProvider) => {
    setEmbeddingProvider(provider);
    const defaultModel = EMBEDDING_MODELS.find((m) => m.provider === provider);
    if (defaultModel) setEmbeddingModel(defaultModel.id);
  };

  const getEmbedding = async (): Promise<number[]> => {
    const result = await embed(query, embeddingProvider, apiKey, embeddingModel);
    return result.vector;
  };

  // ─── RAG Test ─────────────────────────────────────────────────
  const handleRAGTest = async () => {
    if (!selectedCollection || !query || !apiKey) {
      setError('Please fill in all fields');
      return;
    }
    setIsLoading(true);
    setError(null);
    setSuccess(false);
    setResults([]);
    setAiResponse('');
    setTimings(null);

    const totalStart = performance.now();
    try {
      const embeddingStart = performance.now();
      const embedding = await getEmbedding();
      const embeddingMs = Math.round(performance.now() - embeddingStart);

      const searchStart = performance.now();
      const searchResults = await pointsApi.search(selectedCollection, embedding, limit);
      const searchMs = Math.round(performance.now() - searchStart);
      setResults(searchResults);

      const ragPrompt = buildRAGPrompt(query, searchResults);

      const llmStart = performance.now();
      let response: string;
      switch (llmProvider) {
        case 'openai':
          response = await callOpenAI(ragPrompt, apiKey, llmModel);
          break;
        case 'anthropic':
          response = await callAnthropic(ragPrompt, apiKey, llmModel);
          break;
        case 'gemini':
          response = await callGemini(ragPrompt, apiKey, llmModel);
          break;
      }
      const llmMs = Math.round(performance.now() - llmStart);
      const totalMs = Math.round(performance.now() - totalStart);

      setAiResponse(response);
      setTimings({ embeddingMs, searchMs, llmMs, totalMs });
      setSuccess(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'An error occurred');
    } finally {
      setIsLoading(false);
    }
  };

  // ─── Hybrid Search ────────────────────────────────────────────
  const handleHybridSearch = async () => {
    if (!selectedCollection || !query || !apiKey) {
      setHybridError('Please fill in collection, query, and API key');
      return;
    }
    setHybridLoading(true);
    setHybridError(null);
    setHybridResults([]);
    try {
      const embedding = await getEmbedding();
      const params: Record<string, unknown> = {
        query_vector: embedding,
        query_text: hybridTextQuery || query,
        limit,
        fusion: hybridFusion,
      };
      if (hybridFusion === 'weighted') {
        params.alpha = parseFloat(hybridAlpha) || 0.5;
      } else {
        params.rrf_k = parseInt(hybridRrfK, 10) || 60;
      }
      const res = await pointsApi.hybridSearch(selectedCollection, params as any);
      setHybridResults(res);
    } catch (err) {
      setHybridError(err instanceof Error ? err.message : 'Hybrid search failed');
    } finally {
      setHybridLoading(false);
    }
  };

  // ─── Explain ──────────────────────────────────────────────────
  const handleExplain = async () => {
    if (!selectedCollection || !query || !apiKey) {
      setExplainError('Please fill in collection, query, and API key');
      return;
    }
    setExplainLoading(true);
    setExplainError(null);
    setExplainResult(null);
    try {
      const embedding = await getEmbedding();
      const res = await pointsApi.explain(selectedCollection, {
        vector: embedding,
        limit,
      });
      setExplainResult(res);
    } catch (err) {
      setExplainError(err instanceof Error ? err.message : 'Explain failed');
    } finally {
      setExplainLoading(false);
    }
  };

  // ─── Estimate ─────────────────────────────────────────────────
  const handleEstimate = async () => {
    if (!selectedCollection || !query || !apiKey) {
      setEstimateError('Please fill in collection, query, and API key');
      return;
    }
    setEstimateLoading(true);
    setEstimateError(null);
    setEstimateResult(null);
    try {
      const embedding = await getEmbedding();
      const res = await pointsApi.estimate(selectedCollection, {
        vector: embedding,
        limit,
      });
      setEstimateResult(res);
    } catch (err) {
      setEstimateError(err instanceof Error ? err.message : 'Estimate failed');
    } finally {
      setEstimateLoading(false);
    }
  };

  const embeddingModels = EMBEDDING_MODELS.filter((m) => m.provider === embeddingProvider);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-gray-50">Query Tester</h1>
        <p className="text-gray-400 mt-2">
          Test RAG queries, hybrid search, explain results, and estimate costs
        </p>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* ─── Configuration Panel ─────────────────────────────── */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Sparkles className="h-5 w-5" />
              Configuration
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {/* LLM Provider */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">LLM Provider</label>
              <div className="grid grid-cols-3 gap-2">
                {(['openai', 'anthropic', 'gemini'] as LlmProvider[]).map((p) => (
                  <button
                    key={p}
                    onClick={() => handleLlmProviderChange(p)}
                    className={`px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                      llmProvider === p
                        ? 'bg-orange-500 text-white'
                        : 'bg-bg-tertiary text-gray-400 hover:bg-bg-tertiary/80'
                    }`}
                  >
                    {p.charAt(0).toUpperCase() + p.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            {/* LLM Model */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">LLM Model</label>
              <select
                value={llmModel}
                onChange={(e) => setLlmModel(e.target.value)}
                className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
              >
                {LLM_MODELS[llmProvider].map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </div>

            {/* Embedding Provider */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Embedding Provider
              </label>
              <div className="grid grid-cols-2 gap-2">
                {(['openai', 'gemini'] as EmbeddingProvider[]).map((p) => (
                  <button
                    key={p}
                    onClick={() => handleEmbeddingProviderChange(p)}
                    className={`px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                      embeddingProvider === p
                        ? 'bg-orange-500 text-white'
                        : 'bg-bg-tertiary text-gray-400 hover:bg-bg-tertiary/80'
                    }`}
                  >
                    {p === 'openai' ? 'OpenAI' : 'Gemini'}
                  </button>
                ))}
              </div>
            </div>

            {/* Embedding Model */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Embedding Model
              </label>
              <select
                value={embeddingModel}
                onChange={(e) => setEmbeddingModel(e.target.value)}
                className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
              >
                {embeddingModels.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name} ({m.dimensions}d)
                  </option>
                ))}
              </select>
            </div>

            {/* API Key */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">API Key</label>
              <Input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="API key (shared for embedding + LLM)"
                className="bg-bg-secondary border-bg-tertiary text-gray-50"
              />
              {llmProvider === 'anthropic' && (
                <p className="text-xs text-yellow-400 mt-1">
                  Note: Anthropic does not provide embedding API. Use a separate embedding provider
                  above.
                </p>
              )}
            </div>

            {/* Collection */}
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
                        {c.name} ({c.num_points ?? c.point_count ?? 0} points)
                      </option>
                    ))}
                </select>
              )}
            </div>

            {/* Query */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">Query Text</label>
              <textarea
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Enter your query text..."
                rows={4}
                className="w-full rounded-md border border-bg-tertiary bg-bg-secondary px-3 py-2 text-sm text-gray-50 placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-orange-500"
              />
            </div>

            {/* Limit */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">Results Limit</label>
              <Input
                type="number"
                value={limit}
                onChange={(e) => setLimit(Number(e.target.value))}
                min={1}
                max={100}
                className="bg-bg-secondary border-bg-tertiary text-gray-50"
              />
            </div>
          </CardContent>
        </Card>

        {/* ─── Results Panel ───────────────────────────────────── */}
        <Card>
          <CardHeader>
            <CardTitle>Results</CardTitle>
          </CardHeader>
          <CardContent>
            <Tabs value={mainTab} onValueChange={setMainTab}>
              <TabsList className="w-full mb-4">
                <TabsTrigger value="rag">RAG</TabsTrigger>
                <TabsTrigger value="hybrid">Hybrid</TabsTrigger>
                <TabsTrigger value="explain">Explain</TabsTrigger>
                <TabsTrigger value="estimate">Estimate</TabsTrigger>
              </TabsList>

              {/* ─── RAG Tab ──────────────────────────── */}
              <TabsContent value="rag">
                <div className="space-y-4">
                  <Button
                    onClick={handleRAGTest}
                    disabled={isLoading || !selectedCollection || !query || !apiKey}
                    className="w-full"
                    variant="primary"
                  >
                    {isLoading ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Testing...
                      </>
                    ) : (
                      <>
                        <Sparkles className="h-4 w-4 mr-2" />
                        Test RAG Query
                      </>
                    )}
                  </Button>

                  {error && (
                    <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                      <XCircle className="h-4 w-4 text-red-500" />
                      <p className="text-sm text-red-400">{error}</p>
                    </div>
                  )}
                  {success && !isLoading && (
                    <div className="flex items-center gap-2 p-3 rounded-md bg-green-500/10 border border-green-500/50">
                      <CheckCircle2 className="h-4 w-4 text-green-500" />
                      <p className="text-sm text-green-400">Query executed successfully!</p>
                    </div>
                  )}

                  {timings && (
                    <>
                      <div className="flex items-center gap-2">
                        <Clock className="h-5 w-5 text-gray-400" />
                        <h3 className="text-lg font-semibold text-gray-50">Timing</h3>
                      </div>
                      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                          <p className="text-xs text-gray-400">Embedding</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {timings.embeddingMs} ms
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                          <p className="text-xs text-gray-400">Search</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {timings.searchMs} ms
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                          <p className="text-xs text-gray-400">Model (LLM)</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {timings.llmMs} ms
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-orange-500/30">
                          <p className="text-xs text-gray-400">Total</p>
                          <p className="text-sm font-semibold text-orange-500">
                            {timings.totalMs} ms
                          </p>
                        </div>
                      </div>
                    </>
                  )}

                  {aiResponse && (
                    <div>
                      <div className="flex items-center gap-2 mb-3">
                        <Bot className="h-5 w-5 text-orange-500" />
                        <h3 className="text-lg font-semibold text-gray-50">AI Response</h3>
                      </div>
                      <div className="bg-bg-tertiary rounded-lg p-4 border border-bg-tertiary">
                        <p className="text-sm text-gray-200 whitespace-pre-wrap">{aiResponse}</p>
                      </div>
                    </div>
                  )}

                  <SearchResultsList results={results} isLoading={isLoading} />
                </div>
              </TabsContent>

              {/* ─── Hybrid Tab ───────────────────────── */}
              <TabsContent value="hybrid">
                <div className="space-y-4">
                  <div>
                    <label className="block text-sm font-medium text-gray-400 mb-2">
                      Text Query (BM25)
                    </label>
                    <Input
                      value={hybridTextQuery}
                      onChange={(e) => setHybridTextQuery(e.target.value)}
                      placeholder="Optional text query for BM25 (uses main query if empty)"
                      className="bg-bg-secondary border-bg-tertiary text-gray-50"
                    />
                  </div>

                  {/* Fusion Strategy Selector */}
                  <div>
                    <label className="block text-sm font-medium text-gray-400 mb-2">
                      Fusion Strategy
                    </label>
                    <div className="flex gap-2">
                      <button
                        type="button"
                        onClick={() => setHybridFusion('weighted')}
                        className={`flex-1 px-3 py-2 rounded-md text-sm font-medium transition-colors ${
                          hybridFusion === 'weighted'
                            ? 'bg-orange-500/20 text-orange-400 border border-orange-500/50'
                            : 'bg-bg-secondary text-gray-400 border border-bg-tertiary hover:border-gray-500'
                        }`}
                      >
                        Weighted
                      </button>
                      <button
                        type="button"
                        onClick={() => setHybridFusion('rrf')}
                        className={`flex-1 px-3 py-2 rounded-md text-sm font-medium transition-colors ${
                          hybridFusion === 'rrf'
                            ? 'bg-orange-500/20 text-orange-400 border border-orange-500/50'
                            : 'bg-bg-secondary text-gray-400 border border-bg-tertiary hover:border-gray-500'
                        }`}
                      >
                        RRF
                      </button>
                    </div>
                    <p className="text-[10px] text-gray-500 mt-1">
                      {hybridFusion === 'weighted'
                        ? 'Weighted: control vector vs keyword balance with alpha'
                        : 'RRF: stable rank-based fusion, treats all rankers equally'}
                    </p>
                  </div>

                  {/* Alpha slider — only shown for weighted */}
                  {hybridFusion === 'weighted' && (
                    <div>
                      <label className="block text-sm font-medium text-gray-400 mb-2">
                        Alpha ({hybridAlpha}) — 0 = pure BM25, 1 = pure vector
                      </label>
                      <input
                        type="range"
                        min="0"
                        max="1"
                        step="0.05"
                        value={hybridAlpha}
                        onChange={(e) => setHybridAlpha(e.target.value)}
                        className="w-full accent-orange-500"
                      />
                      <div className="flex justify-between text-[10px] text-gray-500">
                        <span>BM25</span>
                        <span>Vector</span>
                      </div>
                    </div>
                  )}

                  {/* RRF k input — only shown for rrf */}
                  {hybridFusion === 'rrf' && (
                    <div>
                      <label className="block text-sm font-medium text-gray-400 mb-2">
                        RRF k ({hybridRrfK}) — higher values smooth rank differences
                      </label>
                      <Input
                        type="number"
                        min="1"
                        value={hybridRrfK}
                        onChange={(e) => setHybridRrfK(e.target.value)}
                        placeholder="60"
                        className="bg-bg-secondary border-bg-tertiary text-gray-50"
                      />
                    </div>
                  )}

                  <Button
                    onClick={handleHybridSearch}
                    disabled={hybridLoading || !selectedCollection || !query || !apiKey}
                    className="w-full"
                    variant="primary"
                  >
                    {hybridLoading ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Searching...
                      </>
                    ) : (
                      <>
                        <Search className="h-4 w-4 mr-2" />
                        Hybrid Search
                      </>
                    )}
                  </Button>

                  {hybridError && (
                    <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                      <XCircle className="h-4 w-4 text-red-500" />
                      <p className="text-sm text-red-400">{hybridError}</p>
                    </div>
                  )}

                  <SearchResultsList results={hybridResults} isLoading={hybridLoading} />
                </div>
              </TabsContent>

              {/* ─── Explain Tab ──────────────────────── */}
              <TabsContent value="explain">
                <div className="space-y-4">
                  <Button
                    onClick={handleExplain}
                    disabled={explainLoading || !selectedCollection || !query || !apiKey}
                    className="w-full"
                    variant="primary"
                  >
                    {explainLoading ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Explaining...
                      </>
                    ) : (
                      <>
                        <FileSearch className="h-4 w-4 mr-2" />
                        Explain Search
                      </>
                    )}
                  </Button>

                  {explainError && (
                    <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                      <XCircle className="h-4 w-4 text-red-500" />
                      <p className="text-sm text-red-400">{explainError}</p>
                    </div>
                  )}

                  {explainResult && (
                    <div className="space-y-4">
                      {/* Explain Summary */}
                      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">Vector Norm</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.query_vector_norm.toFixed(4)}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">Distance</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.distance_metric}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">Scanned</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.candidates_scanned}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">After Filter</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.candidates_after_filter}
                          </p>
                        </div>
                      </div>

                      {/* Index Stats */}
                      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">Total Points</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.index_stats.total_points}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">HNSW Layers</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.index_stats.hnsw_layers}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">ef_search</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.index_stats.ef_search_used}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                          <p className="text-xs text-gray-400">Tombstones</p>
                          <p className="text-sm font-semibold text-gray-50">
                            {explainResult.index_stats.tombstones_skipped}
                          </p>
                        </div>
                      </div>

                      {/* Explained Results */}
                      <div className="space-y-2">
                        <h4 className="text-sm font-semibold text-gray-50">Detailed Results</h4>
                        {explainResult.results.map((r, i) => (
                          <div
                            key={r.id}
                            className="bg-bg-tertiary rounded-lg p-3 border border-bg-tertiary"
                          >
                            <div className="flex items-center justify-between mb-1">
                              <div className="flex items-center gap-2">
                                <Badge variant="default" className="text-xs">
                                  #{i + 1}
                                </Badge>
                                <span className="text-xs font-mono text-gray-400">{r.id}</span>
                              </div>
                            </div>
                            <div className="grid grid-cols-2 sm:grid-cols-4 gap-2 mt-2">
                              <div>
                                <p className="text-[10px] text-gray-500">Score</p>
                                <p className="text-xs font-semibold text-gray-200">
                                  {r.score.toFixed(6)}
                                </p>
                              </div>
                              <div>
                                <p className="text-[10px] text-gray-500">Raw Distance</p>
                                <p className="text-xs font-semibold text-gray-200">
                                  {r.raw_distance.toFixed(6)}
                                </p>
                              </div>
                              <div>
                                <p className="text-[10px] text-gray-500">Rank Before Filter</p>
                                <p className="text-xs font-semibold text-gray-200">
                                  {r.rank_before_filter}
                                </p>
                              </div>
                              <div>
                                <p className="text-[10px] text-gray-500">Rank After Filter</p>
                                <p className="text-xs font-semibold text-gray-200">
                                  {r.rank_after_filter}
                                </p>
                              </div>
                            </div>
                            {/* Score Breakdown */}
                            {r.score_breakdown && Object.keys(r.score_breakdown).length > 0 && (
                              <div className="mt-2 flex gap-2 flex-wrap">
                                {Object.entries(r.score_breakdown).map(([key, val]) => (
                                  <div key={key} className="bg-bg-secondary rounded px-2 py-1">
                                    <p className="text-[10px] text-gray-500">{key}</p>
                                    <p className="text-xs font-semibold text-gray-200">
                                      {val.toFixed(6)}
                                    </p>
                                  </div>
                                ))}
                              </div>
                            )}
                            {/* Filter Evaluation */}
                            {r.filter_evaluation && (
                              <div className="mt-2 bg-bg-secondary rounded p-2">
                                <p className="text-[10px] text-gray-500 mb-1">
                                  Filter: {r.filter_evaluation.passed ? 'Passed' : 'Failed'}
                                </p>
                                {r.filter_evaluation.conditions.map((c, ci) => (
                                  <p key={ci} className="text-[10px] text-gray-400">
                                    {c.field} {c.operator} {JSON.stringify(c.expected)} = {JSON.stringify(c.actual)}{' '}
                                    <span className={c.passed ? 'text-green-400' : 'text-red-400'}>
                                      {c.passed ? 'pass' : 'fail'}
                                    </span>
                                  </p>
                                ))}
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    </div>
                  )}

                  {!explainResult && !explainLoading && !explainError && (
                    <div className="text-center py-12">
                      <FileSearch className="h-12 w-12 mx-auto text-gray-600 mb-4" />
                      <p className="text-gray-400">
                        Click "Explain Search" to see detailed score breakdowns and ranking info.
                      </p>
                    </div>
                  )}
                </div>
              </TabsContent>

              {/* ─── Estimate Tab ─────────────────────── */}
              <TabsContent value="estimate">
                <div className="space-y-4">
                  <Button
                    onClick={handleEstimate}
                    disabled={estimateLoading || !selectedCollection || !query || !apiKey}
                    className="w-full"
                    variant="primary"
                  >
                    {estimateLoading ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Estimating...
                      </>
                    ) : (
                      <>
                        <Gauge className="h-4 w-4 mr-2" />
                        Estimate Cost
                      </>
                    )}
                  </Button>

                  {estimateError && (
                    <div className="flex items-center gap-2 p-3 rounded-md bg-red-500/10 border border-red-500/50">
                      <XCircle className="h-4 w-4 text-red-500" />
                      <p className="text-sm text-red-400">{estimateError}</p>
                    </div>
                  )}

                  {estimateResult && (
                    <div className="space-y-4">
                      {/* Main stats */}
                      <div className="grid grid-cols-2 gap-3">
                        <div className="bg-bg-tertiary rounded-lg px-4 py-3">
                          <p className="text-xs text-gray-400">Estimated Latency</p>
                          <p className="text-xl font-bold text-orange-500">
                            {estimateResult.estimated_ms.toFixed(2)} ms
                          </p>
                          <p className="text-[10px] text-gray-500">
                            range: {estimateResult.confidence_range[0].toFixed(2)} – {estimateResult.confidence_range[1].toFixed(2)} ms
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-4 py-3">
                          <p className="text-xs text-gray-400">Memory Usage</p>
                          <p className="text-xl font-bold text-gray-50">
                            {(estimateResult.estimated_memory_bytes / 1024).toFixed(1)} KB
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-4 py-3">
                          <p className="text-xs text-gray-400">Nodes to Visit</p>
                          <p className="text-xl font-bold text-gray-50">
                            {estimateResult.estimated_nodes_visited}
                          </p>
                        </div>
                        <div className="bg-bg-tertiary rounded-lg px-4 py-3">
                          <p className="text-xs text-gray-400">Expensive?</p>
                          <p className={`text-xl font-bold ${estimateResult.is_expensive ? 'text-red-400' : 'text-green-400'}`}>
                            {estimateResult.is_expensive ? 'Yes' : 'No'}
                          </p>
                        </div>
                      </div>

                      {/* Cost Breakdown */}
                      <div>
                        <h4 className="text-sm font-semibold text-gray-50 mb-2">Cost Breakdown</h4>
                        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                          <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                            <p className="text-xs text-gray-400">Index Scan</p>
                            <p className="text-sm font-semibold text-gray-50">
                              {estimateResult.breakdown.index_scan_cost.toFixed(4)} ms
                            </p>
                          </div>
                          <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                            <p className="text-xs text-gray-400">Filter</p>
                            <p className="text-sm font-semibold text-gray-50">
                              {estimateResult.breakdown.filter_cost.toFixed(4)} ms
                            </p>
                          </div>
                          <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                            <p className="text-xs text-gray-400">Hydration</p>
                            <p className="text-sm font-semibold text-gray-50">
                              {estimateResult.breakdown.hydration_cost.toFixed(4)} ms
                            </p>
                          </div>
                          <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                            <p className="text-xs text-gray-400">Network</p>
                            <p className="text-sm font-semibold text-gray-50">
                              {estimateResult.breakdown.network_overhead.toFixed(4)} ms
                            </p>
                          </div>
                        </div>
                      </div>

                      {/* Recommendations */}
                      {estimateResult.recommendations.length > 0 && (
                        <div>
                          <h4 className="text-sm font-semibold text-gray-50 mb-2">Recommendations</h4>
                          <ul className="space-y-1">
                            {estimateResult.recommendations.map((rec, i) => (
                              <li key={i} className="text-sm text-yellow-400 bg-yellow-500/10 border border-yellow-500/30 rounded px-3 py-2">
                                {rec}
                              </li>
                            ))}
                          </ul>
                        </div>
                      )}

                      {/* Historical Latency */}
                      {estimateResult.historical_latency && (
                        <div>
                          <h4 className="text-sm font-semibold text-gray-50 mb-2">
                            Historical Latency ({estimateResult.historical_latency.total_queries} queries)
                          </h4>
                          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                            <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                              <p className="text-xs text-gray-400">Avg</p>
                              <p className="text-sm font-semibold text-gray-50">
                                {estimateResult.historical_latency.avg_ms.toFixed(1)} ms
                              </p>
                            </div>
                            <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                              <p className="text-xs text-gray-400">P50</p>
                              <p className="text-sm font-semibold text-gray-50">
                                {estimateResult.historical_latency.p50_ms.toFixed(1)} ms
                              </p>
                            </div>
                            <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                              <p className="text-xs text-gray-400">P95</p>
                              <p className="text-sm font-semibold text-gray-50">
                                {estimateResult.historical_latency.p95_ms.toFixed(1)} ms
                              </p>
                            </div>
                            <div className="bg-bg-tertiary rounded-lg px-3 py-2">
                              <p className="text-xs text-gray-400">P99</p>
                              <p className="text-sm font-semibold text-gray-50">
                                {estimateResult.historical_latency.p99_ms.toFixed(1)} ms
                              </p>
                            </div>
                          </div>
                        </div>
                      )}
                    </div>
                  )}

                  {!estimateResult && !estimateLoading && !estimateError && (
                    <div className="text-center py-12">
                      <Gauge className="h-12 w-12 mx-auto text-gray-600 mb-4" />
                      <p className="text-gray-400">
                        Click "Estimate Cost" to see pre-flight cost analysis before running a
                        query.
                      </p>
                    </div>
                  )}
                </div>
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

// ─── Shared Search Results List ───────────────────────────────────────

function SearchResultsList({
  results,
  isLoading,
}: {
  results: SearchResult[];
  isLoading: boolean;
}) {
  return (
    <div>
      <h3 className="text-lg font-semibold text-gray-50 mb-3">
        Search Results {results.length > 0 && `(${results.length})`}
      </h3>
      {isLoading ? (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-24 w-full" />
          ))}
        </div>
      ) : results.length > 0 ? (
        <div className="space-y-4">
          {results.map((result, index) => (
            <Card key={result.id} className="bg-bg-tertiary border-bg-tertiary">
              <CardContent className="p-4">
                <div className="flex items-start justify-between mb-2">
                  <div className="flex items-center gap-2">
                    <Badge variant="default" className="text-xs">
                      #{index + 1}
                    </Badge>
                    <span className="text-xs text-gray-400">Score: {result.score.toFixed(4)}</span>
                  </div>
                  <span className="text-xs font-mono text-gray-400">{result.id}</span>
                </div>
                {result.metadata && (
                  <div className="mt-3">
                    {Object.entries(result.metadata).map(([key, value]) => {
                      if (key === 'text' || key === 'content' || key === 'body') {
                        return (
                          <div key={key} className="mt-2">
                            <p className="text-xs text-gray-400 mb-1">{key}:</p>
                            <p className="text-sm text-gray-200 bg-bg-secondary p-2 rounded line-clamp-3">
                              {String(value)}
                            </p>
                          </div>
                        );
                      }
                      return (
                        <div key={key} className="text-xs text-gray-400 mt-1">
                          <span className="font-medium">{key}:</span> {String(value)}
                        </div>
                      );
                    })}
                  </div>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <div className="text-center py-12">
          <p className="text-gray-400">No results yet. Run a query to see results here.</p>
        </div>
      )}
    </div>
  );
}
