import { useState, useCallback } from "react";
import { llmApi } from "@/api/ferresdb";
import type { EmbeddingProvider, EmbeddingResult } from "@/types";

export function useEmbedding() {
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const embed = useCallback(
    async (
      text: string,
      provider: EmbeddingProvider,
      model?: string,
    ): Promise<EmbeddingResult> => {
      setIsLoading(true);
      setError(null);
      const start = performance.now();

      try {
        const usedModel =
          model || (provider === "openai" ? "text-embedding-3-small" : "text-embedding-004");
        const result = await llmApi.embed({ provider, model: usedModel, input: text });
        const vector = result.vectors[0] ?? [];
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
      model?: string,
      onProgress?: (done: number, total: number) => void,
    ): Promise<EmbeddingResult[]> => {
      setIsLoading(true);
      setError(null);
      const start = performance.now();

      try {
        const usedModel =
          model || (provider === "openai" ? "text-embedding-3-small" : "text-embedding-004");

        const CHUNK_SIZE = 100;
        const allVectors: number[][] = [];

        for (let i = 0; i < texts.length; i += CHUNK_SIZE) {
          const chunk = texts.slice(i, i + CHUNK_SIZE);
          const result = await llmApi.embed({ provider, model: usedModel, input: chunk });
          allVectors.push(...result.vectors);
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
