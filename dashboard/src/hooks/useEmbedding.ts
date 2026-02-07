import { useState, useCallback } from "react";
import type { EmbeddingProvider, EmbeddingResult } from "@/types";

// ─── OpenAI Embedding ────────────────────────────────────────────────

async function embedOpenAI(
  text: string,
  apiKey: string,
  model: string = "text-embedding-3-small",
): Promise<number[]> {
  const response = await fetch("https://api.openai.com/v1/embeddings", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ model, input: text }),
  });

  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || `OpenAI embedding failed (${response.status})`);
  }

  const data = await response.json();
  return data.data[0].embedding;
}

// ─── Gemini Embedding ────────────────────────────────────────────────

async function embedGemini(
  text: string,
  apiKey: string,
  model: string = "text-embedding-004",
): Promise<number[]> {
  const url = `https://generativelanguage.googleapis.com/v1beta/models/${model}:embedContent?key=${apiKey}`;
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model: `models/${model}`,
      content: { parts: [{ text }] },
    }),
  });

  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || `Gemini embedding failed (${response.status})`);
  }

  const data = await response.json();
  return data.embedding.values;
}

// ─── Batch helpers (serial with delay to respect rate limits) ─────────

async function embedBatchOpenAI(
  texts: string[],
  apiKey: string,
  model: string = "text-embedding-3-small",
): Promise<number[][]> {
  // OpenAI supports batch input natively
  const response = await fetch("https://api.openai.com/v1/embeddings", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ model, input: texts }),
  });

  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || `OpenAI batch embedding failed (${response.status})`);
  }

  const data = await response.json();
  // Sort by index to preserve order
  const sorted = [...data.data].sort((a: any, b: any) => a.index - b.index);
  return sorted.map((item: any) => item.embedding);
}

async function embedBatchGemini(
  texts: string[],
  apiKey: string,
  model: string = "text-embedding-004",
): Promise<number[][]> {
  // Gemini batchEmbedContents
  const url = `https://generativelanguage.googleapis.com/v1beta/models/${model}:batchEmbedContents?key=${apiKey}`;
  const requests = texts.map((text) => ({
    model: `models/${model}`,
    content: { parts: [{ text }] },
  }));

  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ requests }),
  });

  if (!response.ok) {
    const err = await response.json().catch(() => ({}));
    throw new Error(err.error?.message || `Gemini batch embedding failed (${response.status})`);
  }

  const data = await response.json();
  return data.embeddings.map((e: any) => e.values);
}

// ─── Hook ────────────────────────────────────────────────────────────

export function useEmbedding() {
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const embed = useCallback(
    async (
      text: string,
      provider: EmbeddingProvider,
      apiKey: string,
      model?: string,
    ): Promise<EmbeddingResult> => {
      setIsLoading(true);
      setError(null);
      const start = performance.now();

      try {
        let vector: number[];
        const usedModel =
          model || (provider === "openai" ? "text-embedding-3-small" : "text-embedding-004");

        if (provider === "openai") {
          vector = await embedOpenAI(text, apiKey, usedModel);
        } else {
          vector = await embedGemini(text, apiKey, usedModel);
        }

        const took_ms = Math.round(performance.now() - start);
        return { vector, dimensions: vector.length, model: usedModel, provider, took_ms };
      } catch (err) {
        const msg = err instanceof Error ? err.message : "Embedding failed";
        setError(msg);
        throw err;
      } finally {
        setIsLoading(false);
      }
    },
    [],
  );

  const embedBatch = useCallback(
    async (
      texts: string[],
      provider: EmbeddingProvider,
      apiKey: string,
      model?: string,
      onProgress?: (done: number, total: number) => void,
    ): Promise<EmbeddingResult[]> => {
      setIsLoading(true);
      setError(null);
      const start = performance.now();

      try {
        const usedModel =
          model || (provider === "openai" ? "text-embedding-3-small" : "text-embedding-004");

        // Process in chunks of 100 to avoid huge payloads
        const CHUNK_SIZE = 100;
        const allVectors: number[][] = [];

        for (let i = 0; i < texts.length; i += CHUNK_SIZE) {
          const chunk = texts.slice(i, i + CHUNK_SIZE);
          let vectors: number[][];

          if (provider === "openai") {
            vectors = await embedBatchOpenAI(chunk, apiKey, usedModel);
          } else {
            vectors = await embedBatchGemini(chunk, apiKey, usedModel);
          }

          allVectors.push(...vectors);
          onProgress?.(allVectors.length, texts.length);
        }

        const took_ms = Math.round(performance.now() - start);
        return allVectors.map((vector) => ({
          vector,
          dimensions: vector.length,
          model: usedModel,
          provider,
          took_ms,
        }));
      } catch (err) {
        const msg = err instanceof Error ? err.message : "Batch embedding failed";
        setError(msg);
        throw err;
      } finally {
        setIsLoading(false);
      }
    },
    [],
  );

  return { embed, embedBatch, isLoading, error, clearError: () => setError(null) };
}
