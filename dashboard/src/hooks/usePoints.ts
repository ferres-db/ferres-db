import { useQuery } from '@tanstack/react-query';
import { pointsApi } from '@/api/ferresdb';

export interface UsePointsOptions {
  limit?: number;
  offset?: number;
  filter?: Record<string, unknown>;
}

export const usePoints = (collectionName: string, options?: UsePointsOptions) => {
  return useQuery({
    queryKey: ['points', collectionName, options],
    queryFn: () => pointsApi.list(collectionName, options),
    enabled: !!collectionName,
  });
};
