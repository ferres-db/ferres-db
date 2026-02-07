import { useQuery } from "@tanstack/react-query";
import { auditApi } from "@/api/ferresdb";
import type { AuditQueryParams } from "@/types";

export const useAudit = (
  params: AuditQueryParams & { enabled?: boolean } = {},
) => {
  const { enabled = true, ...queryParams } = params;
  return useQuery({
    queryKey: ["audit", queryParams],
    queryFn: () => auditApi.list(queryParams),
    enabled: enabled ?? true,
  });
};
