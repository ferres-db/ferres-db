import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { keysApi } from "@/api/ferresdb";
import type { CreateApiKeyResponse } from "@/types";

export const useApiKeys = () => {
  return useQuery({
    queryKey: ["api-keys"],
    queryFn: keysApi.list,
  });
};

export const useCreateApiKey = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      name,
      allowed_namespaces,
    }: {
      name: string;
      allowed_namespaces?: string[] | null;
    }) => keysApi.create(name, allowed_namespaces),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
};

export const useUpdateKeyNamespaces = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      id,
      allowed_namespaces,
    }: {
      id: number;
      allowed_namespaces: string[] | null;
    }) => keysApi.updateNamespaces(id, allowed_namespaces),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
};

export const useDeleteApiKey = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: number) => keysApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });
};

export type { CreateApiKeyResponse };
