import { useQuery } from '@tanstack/react-query';
import { statsApi } from '@/api/ferresdb';
import type { CollectionStats, QueryEntry } from '@/types';

export const useCollectionStats = (collectionName: string) => {
  return useQuery<CollectionStats>({
    queryKey: ['collectionStats', collectionName],
    queryFn: () => statsApi.collection(collectionName),
    enabled: !!collectionName,
  });
};

export const useCollectionQueries = (collectionName: string) => {
  return useQuery<QueryEntry[]>({
    queryKey: ['collectionQueries', collectionName],
    queryFn: async () => {
      return await statsApi.queries(collectionName);
    },
    enabled: !!collectionName,
  });
};
