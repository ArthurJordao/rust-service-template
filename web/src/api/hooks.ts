import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { getMe } from "@/api/accounts";
import { listScopes, listUsers, setUserScopes } from "@/api/users";
import { listDeadLetters, replayDeadLetter } from "@/api/dlq";

export function useMe(enabled: boolean) {
  return useQuery({ queryKey: ["me"], queryFn: getMe, enabled });
}
export function useUsers() {
  return useQuery({ queryKey: ["users"], queryFn: listUsers });
}
export function useScopes() {
  return useQuery({ queryKey: ["scopes"], queryFn: listScopes });
}
export function useSetUserScopes() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, scopes }: { id: number; scopes: string[] }) => setUserScopes(id, scopes),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ["users"] }); toast.success("Scopes updated"); },
    onError: () => toast.error("Failed to update scopes"),
  });
}
export function useDeadLetters() {
  return useQuery({ queryKey: ["dlq"], queryFn: listDeadLetters });
}
export function useReplayDeadLetter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (deliveryId: number) => replayDeadLetter(deliveryId),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ["dlq"] }); toast.success("Replayed"); },
    onError: () => toast.error("Replay failed"),
  });
}
