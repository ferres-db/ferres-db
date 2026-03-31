import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { collectionsApi } from "@/api/ferresdb";
import type {
  QuantizationConfig,
  TieredStorageConfig,
  TierDistribution,
} from "@/types";

export const useCollections = (options?: { namespace?: string }) => {
  return useQuery({
    queryKey: ["collections", options?.namespace ?? null],
    queryFn: () => collectionsApi.list(options),
  });
};

export const useCollection = (name: string) => {
  return useQuery({
    queryKey: ["collections", name],
    queryFn: () => collectionsApi.get(name),
    enabled: !!name,
  });
};

export const useTierDistribution = (name: string) => {
  return useQuery<TierDistribution>({
    queryKey: ["tierDistribution", name],
    queryFn: () => collectionsApi.getTierDistribution(name),
    enabled: !!name,
  });
};

export const useCreateCollection = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      name,
      vectorSize,
      distanceMetric,
      quantization,
      enable_bm25,
      bm25_text_field,
      tiered_storage,
    }: {
      name: string;
      vectorSize: number;
      distanceMetric?: string;
      quantization?: QuantizationConfig;
      enable_bm25?: boolean;
      bm25_text_field?: string;
      tiered_storage?: TieredStorageConfig;
    }) =>
      collectionsApi.create(name, vectorSize, distanceMetric, {
        quantization,
        enable_bm25,
        bm25_text_field,
        tiered_storage,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["collections"] });
    },
  });
};

export const useDeleteCollection = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (name: string) => collectionsApi.delete(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["collections"] });
    },
  });
};
