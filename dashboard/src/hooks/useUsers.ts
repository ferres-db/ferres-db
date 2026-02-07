import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { usersApi } from "@/api/ferresdb";
import type { Permission } from "@/types";

export const useUsers = () => {
  return useQuery({
    queryKey: ["users"],
    queryFn: usersApi.list,
  });
};

export const useCreateUser = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      username,
      password,
      role,
      permissions,
    }: {
      username: string;
      password: string;
      role?: string;
      permissions?: Permission[] | null;
    }) => usersApi.create(username, password, role, permissions),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["users"] });
    },
  });
};

export const useDeleteUser = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (id: number) => usersApi.delete(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["users"] });
    },
  });
};

export const useUpdateUserPassword = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      username,
      password,
    }: {
      username: string;
      password: string;
    }) => usersApi.updatePassword(username, password),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["users"] });
    },
  });
};

export const useUpdateUserPermissions = () => {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      username,
      permissions,
    }: {
      username: string;
      permissions: Permission[] | null;
    }) => usersApi.updatePermissions(username, permissions),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["users"] });
    },
  });
};
