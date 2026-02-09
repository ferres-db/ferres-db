import { useQuery } from '@tanstack/react-query';
import { statsApi } from '@/api/ferresdb';

export const useGlobalStats = () => {
  return useQuery({
    queryKey: ['stats', 'global'],
    queryFn: statsApi.global,
    refetchInterval: 5000, // Refresh every 5 seconds
  });
};

export const useAnalytics = () => {
  return useQuery({
    queryKey: ['stats', 'analytics'],
    queryFn: statsApi.analytics,
    refetchInterval: 10000, // Refresh every 10 seconds
  });
};

export const useQueryStats = () => {
  return useQuery({
    queryKey: ['stats', 'queries'],
    queryFn: async () => {
      return await statsApi.queries();
    },
    refetchInterval: 5000,
    retry: false, // Não retry em caso de erro para evitar spam
  });
};

export const useSlowQueries = () => {
  return useQuery({
    queryKey: ['stats', 'slow-queries'],
    queryFn: statsApi.slowQueries,
    refetchInterval: 10000, // Refresh every 10 seconds
  });
};

export const useCollectionStats = (collectionName: string) => {
  return useQuery({
    queryKey: ['stats', 'collection', collectionName],
    queryFn: () => statsApi.collection(collectionName),
    enabled: !!collectionName,
    refetchInterval: 5000,
  });
};
