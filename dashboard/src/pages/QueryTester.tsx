import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { getStoredRole } from '@/api/ferresdb';
import { useCollections } from '@/hooks/useCollections';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/Card';
import { Button } from '@/components/ui/Button';
import { Input } from '@/components/ui/Input';
import { Skeleton } from '@/components/ui/Skeleton';
import { Badge } from '@/components/ui/Badge';
import { Sparkles, Loader2, CheckCircle2, XCircle, Bot, Clock } from 'lucide-react';
import { pointsApi } from '@/api/ferresdb';
import type { SearchResult } from '@/types';

type Provider = 'openai' | 'anthropic' | 'gemini';

export const QueryTester = () => {
  const navigate = useNavigate();
  const role = getStoredRole();
  useEffect(() => {
    if (role === 'viewer') navigate('/', { replace: true });
  }, [role, navigate]);
  return <QueryTesterContent />;
};

const PROVIDER_MODELS: Record<Provider, string[]> = {
  openai: [
    'gpt-4o',
    'gpt-4o-mini',
    'gpt-4-turbo',
    'gpt-4',
    'gpt-3.5-turbo',
  ],
  anthropic: [
    'claude-3-5-sonnet-20241022',
    'claude-3-5-haiku-20241022',
    'claude-3-opus-20240229',
    'claude-3-sonnet-20240229',
  ],
  gemini: [
    'gemini-1.5-pro',
    'gemini-1.5-flash',
    'gemini-pro',
  ],
};

const DEFAULT_MODELS: Record<Provider, string> = {
  openai: 'gpt-4o-mini',
  anthropic: 'claude-3-5-haiku-20241022',
  gemini: 'gemini-1.5-flash',
};

function QueryTesterContent() {
  const { data: collections, isLoading: collectionsLoading } = useCollections();
  const [selectedProvider, setSelectedProvider] = useState<Provider>('openai');
  const [selectedModel, setSelectedModel] = useState<string>(DEFAULT_MODELS.openai);
  const [apiKey, setApiKey] = useState('');
  const [selectedCollection, setSelectedCollection] = useState('');
  const [query, setQuery] = useState('');
  const [limit, setLimit] = useState(5);
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

  // Atualizar modelo quando provider mudar
  const handleProviderChange = (provider: Provider) => {
    setSelectedProvider(provider);
    setSelectedModel(DEFAULT_MODELS[provider]);
  };

  // Função para fazer embedding usando OpenAI
  const embedWithOpenAI = async (text: string, apiKey: string): Promise<number[]> => {
    const response = await fetch('https://api.openai.com/v1/embeddings', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model: 'text-embedding-3-small',
        input: text,
      }),
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error?.message || 'Failed to get embedding from OpenAI');
    }

    const data = await response.json();
    return data.data[0].embedding;
  };

  // Função para fazer embedding usando Gemini
  const embedWithGemini = async (text: string, apiKey: string): Promise<number[]> => {
    const response = await fetch(`https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent?key=${apiKey}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        model: 'models/text-embedding-004',
        content: {
          parts: [{ text }],
        },
      }),
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error?.message || 'Failed to get embedding from Gemini');
    }

    const data = await response.json();
    return data.embedding.values;
  };

  // Construir prompt RAG
  const buildRAGPrompt = (question: string, contextChunks: SearchResult[]): string => {
    const contextParts = contextChunks.map((result, index) => {
      const meta = result.metadata || {};
      const text = (meta.text || meta.content || meta.body) as string || '(conteúdo não disponível no metadata)';
      const source = meta.source as string || 'N/A';
      return `[${index + 1}] (fonte: ${source})\n${text}`;
    });

    const context = contextParts.length > 0 
      ? contextParts.join('\n\n---\n\n')
      : '(Nenhum contexto recuperado)';

    return `Contexto da documentação:

${context}

---

Pergunta: ${question}

Responda com base apenas no contexto acima. Se não souber, diga que não encontrou informação.`;
  };

  // Chamar LLM OpenAI
  const callOpenAI = async (prompt: string, apiKey: string, model: string): Promise<string> => {
    const response = await fetch('https://api.openai.com/v1/chat/completions', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${apiKey}`,
      },
      body: JSON.stringify({
        model,
        messages: [{ role: 'user', content: prompt }],
        temperature: 0.7,
      }),
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error?.message || 'Failed to get response from OpenAI');
    }

    const data = await response.json();
    return data.choices[0].message.content || '';
  };

  // Chamar LLM Anthropic
  const callAnthropic = async (prompt: string, apiKey: string, model: string): Promise<string> => {
    const response = await fetch('https://api.anthropic.com/v1/messages', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-api-key': apiKey,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify({
        model,
        max_tokens: 1024,
        messages: [{ role: 'user', content: prompt }],
      }),
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error?.message || 'Failed to get response from Anthropic');
    }

    const data = await response.json();
    return data.content[0].text || '';
  };

  // Chamar LLM Gemini
  const callGemini = async (prompt: string, apiKey: string, model: string): Promise<string> => {
    const response = await fetch(`https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent?key=${apiKey}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        contents: [{
          parts: [{ text: prompt }],
        }],
      }),
    });

    if (!response.ok) {
      const error = await response.json();
      throw new Error(error.error?.message || 'Failed to get response from Gemini');
    }

    const data = await response.json();
    return data.candidates[0].content.parts[0].text || '';
  };

  const handleTest = async () => {
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
      let embedding: number[];

      // Obter embedding baseado no provider
      const embeddingStart = performance.now();
      switch (selectedProvider) {
        case 'openai':
          embedding = await embedWithOpenAI(query, apiKey);
          break;
        case 'anthropic':
          embedding = await embedWithGemini(query, apiKey);
          break;
        case 'gemini':
          embedding = await embedWithGemini(query, apiKey);
          break;
        default:
          throw new Error('Invalid provider');
      }
      const embeddingMs = Math.round(performance.now() - embeddingStart);

      // Buscar na collection
      const searchStart = performance.now();
      const searchResults = await pointsApi.search(selectedCollection, embedding, limit);
      const searchMs = Math.round(performance.now() - searchStart);
      setResults(searchResults);

      // Construir prompt RAG
      const ragPrompt = buildRAGPrompt(query, searchResults);

      // Chamar LLM
      const llmStart = performance.now();
      let response: string;
      switch (selectedProvider) {
        case 'openai':
          response = await callOpenAI(ragPrompt, apiKey, selectedModel);
          break;
        case 'anthropic':
          response = await callAnthropic(ragPrompt, apiKey, selectedModel);
          break;
        case 'gemini':
          response = await callGemini(ragPrompt, apiKey, selectedModel);
          break;
        default:
          throw new Error('Invalid provider');
      }
      const llmMs = Math.round(performance.now() - llmStart);

      const totalMs = Math.round(performance.now() - totalStart);

      setAiResponse(response);
      setTimings({ embeddingMs, searchMs, llmMs, totalMs });
      setSuccess(true);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'An error occurred');
      setSuccess(false);
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-3xl font-bold text-gray-50">Query Tester</h1>
        <p className="text-gray-400 mt-2">Test RAG queries with OpenAI, Anthropic, or Gemini</p>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Configuration Panel */}
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Sparkles className="h-5 w-5" />
              Configuration
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {/* Provider Selection */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                LLM Provider
              </label>
              <div className="grid grid-cols-3 gap-2">
                {(['openai', 'anthropic', 'gemini'] as Provider[]).map((provider) => (
                  <button
                    key={provider}
                    onClick={() => handleProviderChange(provider)}
                    className={`px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                      selectedProvider === provider
                        ? 'bg-orange-500 text-white'
                        : 'bg-bg-tertiary text-gray-400 hover:bg-bg-tertiary/80'
                    }`}
                  >
                    {provider.charAt(0).toUpperCase() + provider.slice(1)}
                  </button>
                ))}
              </div>
              {selectedProvider === 'anthropic' && (
                <p className="text-xs text-yellow-400 mt-2">
                  Note: Anthropic does not provide embedding API. Using Gemini for embeddings.
                </p>
              )}
            </div>

            {/* Model Selection */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Model
              </label>
              <select
                value={selectedModel}
                onChange={(e) => setSelectedModel(e.target.value)}
                className="w-full h-10 rounded-md border border-bg-tertiary bg-bg-secondary px-3 text-sm text-gray-50"
              >
                {PROVIDER_MODELS[selectedProvider].map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            </div>

            {/* API Key */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                API Key
              </label>
              <Input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={`Enter your ${selectedProvider} API key`}
                className="bg-bg-secondary border-bg-tertiary text-gray-50"
              />
            </div>

            {/* Collection Selection */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Collection
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
                  {Array.isArray(collections) && collections.map((collection) => (
                    <option key={collection.name} value={collection.name}>
                      {collection.name} ({collection.num_points ?? collection.point_count ?? 0} points)
                    </option>
                  ))}
                </select>
              )}
            </div>

            {/* Query Input */}
            <div>
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Query Text
              </label>
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
              <label className="block text-sm font-medium text-gray-400 mb-2">
                Results Limit
              </label>
              <Input
                type="number"
                value={limit}
                onChange={(e) => setLimit(Number(e.target.value))}
                min={1}
                max={100}
                className="bg-bg-secondary border-bg-tertiary text-gray-50"
              />
            </div>

            {/* Test Button */}
            <Button
              onClick={handleTest}
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
                  Test Query
                </>
              )}
            </Button>

            {/* Status Messages */}
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
          </CardContent>
        </Card>

        {/* Results Panel */}
        <Card>
          <CardHeader>
            <CardTitle>Results</CardTitle>
          </CardHeader>
          <CardContent className="space-y-6">
            {/* Timings */}
            {timings && (
              <div className="flex items-center gap-2 mb-2">
                <Clock className="h-5 w-5 text-gray-400" />
                <h3 className="text-lg font-semibold text-gray-50">Timing</h3>
              </div>
            )}
            {timings && (
              <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                  <p className="text-xs text-gray-400">Embedding</p>
                  <p className="text-sm font-semibold text-gray-50">{timings.embeddingMs} ms</p>
                </div>
                <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                  <p className="text-xs text-gray-400">Search</p>
                  <p className="text-sm font-semibold text-gray-50">{timings.searchMs} ms</p>
                </div>
                <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-bg-tertiary">
                  <p className="text-xs text-gray-400">Model (LLM)</p>
                  <p className="text-sm font-semibold text-gray-50">{timings.llmMs} ms</p>
                </div>
                <div className="bg-bg-tertiary rounded-lg px-3 py-2 border border-orange-500/30">
                  <p className="text-xs text-gray-400">Total</p>
                  <p className="text-sm font-semibold text-orange-500">{timings.totalMs} ms</p>
                </div>
              </div>
            )}

            {/* AI Response */}
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

            {/* Search Results */}
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
          </CardContent>
        </Card>
      </div>
    </div>
  );
};
