import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { keysApi } from '@/api/ferresdb';
import type { CreateApiKeyResponse } from '@/types';

export const useApiKeys = () => {
  return useQuery({
    queryKey: ['api-keys'],
    queryFn: keysApi.list,
  });
};

export const useCreateApiKey = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (name: string) => keysApi.create(name),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['api-keys'] });
    },
  });
};

export const useDeleteApiKey = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: number) => keysApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['api-keys'] });
    },
  });
};

export type { CreateApiKeyResponse };
