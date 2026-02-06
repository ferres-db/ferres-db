import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { collectionsApi } from '@/api/ferresdb';

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
    mutationFn: ({ name, vectorSize, distanceMetric }: { name: string; vectorSize: number; distanceMetric?: string }) =>
      collectionsApi.create(name, vectorSize, distanceMetric),
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
