import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { collectionsApi } from '@/api/ferresdb';
import type { QuantizationConfig } from '@/types';

export const useCollections = () => {
  return useQuery({
    queryKey: ['collections'],
    queryFn: collectionsApi.list,
  });
};

export const useCollection = (name: string) => {
  return useQuery({
    queryKey: ['collections', name],
    queryFn: () => collectionsApi.get(name),
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
    }: {
      name: string;
      vectorSize: number;
      distanceMetric?: string;
      quantization?: QuantizationConfig;
      enable_bm25?: boolean;
      bm25_text_field?: string;
    }) =>
      collectionsApi.create(name, vectorSize, distanceMetric, {
        quantization,
        enable_bm25,
        bm25_text_field,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['collections'] });
    },
  });
};

export const useDeleteCollection = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (name: string) => collectionsApi.delete(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['collections'] });
    },
  });
};
