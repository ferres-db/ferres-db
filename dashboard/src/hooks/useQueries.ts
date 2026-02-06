import { useQuery, useMutation } from '@tanstack/react-query';
import { pointsApi } from '@/api/ferresdb';
import type { Point, SearchResult } from '@/types';

export const usePoint = (collection: string, id: string) => {
  return useQuery({
    queryKey: ['points', collection, id],
    queryFn: () => pointsApi.get(collection, id),
    enabled: !!collection && !!id,
  });
};

export const useSearch = () => {
  return useMutation({
    mutationFn: ({
      collection,
      vector,
      limit,
      filter,
    }: {
      collection: string;
      vector: number[];
      limit?: number;
      filter?: Record<string, unknown>;
    }): Promise<SearchResult[]> => pointsApi.search(collection, vector, limit, filter),
  });
};

export const useUpsertPoints = () => {
  return useMutation({
    mutationFn: ({ collection, points }: { collection: string; points: Point[] }) =>
      pointsApi.upsert(collection, points),
  });
};

export const useDeletePoints = () => {
  return useMutation({
    mutationFn: ({ collection, ids }: { collection: string; ids: string[] }) =>
      pointsApi.delete(collection, ids),
  });
};
