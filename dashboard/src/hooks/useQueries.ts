import { useQuery, useMutation } from '@tanstack/react-query';
import { pointsApi } from '@/api/ferresdb';
import type { Point, SearchResult } from '@/types';

export const usePoint = (
  collection: string,
  id: string,
  options?: { namespace?: string },
) => {
  return useQuery({
    queryKey: ['points', collection, id, options?.namespace],
    queryFn: () => pointsApi.get(collection, id, options),
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
      namespace,
      vector_field,
    }: {
      collection: string;
      vector: number[];
      limit?: number;
      filter?: Record<string, unknown>;
      namespace?: string;
      vector_field?: string;
    }): Promise<SearchResult[]> =>
      pointsApi.search(collection, vector, limit, filter, { namespace, vector_field }).then((r) => r.results),
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
    mutationFn: ({
      collection,
      ids,
      namespace,
    }: {
      collection: string;
      ids: string[];
      namespace?: string;
    }) => pointsApi.delete(collection, ids, { namespace }),
  });
};
