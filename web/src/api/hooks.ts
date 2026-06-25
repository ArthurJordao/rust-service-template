import { useQuery } from "@tanstack/react-query";
import { getMe } from "@/api/accounts";

export function useMe(enabled: boolean) {
  return useQuery({ queryKey: ["me"], queryFn: getMe, enabled });
}
