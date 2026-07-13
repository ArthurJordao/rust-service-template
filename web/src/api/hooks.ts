import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { refSuffix } from "@/lib/errors";
import { getMe } from "@/api/accounts";
import { listScopes, listUsers, setUserScopes } from "@/api/users";
import { listDeadLetters, replayDeadLetter } from "@/api/dlq";
import * as mfaApi from "@/api/mfa";

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
    onError: (e) => toast.error("Failed to update scopes" + refSuffix(e)),
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
    onError: (e) => toast.error("Replay failed" + refSuffix(e)),
  });
}
export function useMfaStatus(enabled: boolean) {
  return useQuery({ queryKey: ["mfa-status"], queryFn: mfaApi.mfaStatus, enabled });
}
export function useRegenRecoveryCodes() {
  return useMutation({
    mutationFn: () => mfaApi.regenRecoveryCodes(),
    onError: (e) => toast.error("Could not regenerate codes" + refSuffix(e)),
  });
}
export function useDisableMfa() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => mfaApi.disableMfa(),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ["mfa-status"] }); toast.success("MFA disabled"); },
    onError: (e) => toast.error("Could not disable MFA" + refSuffix(e)),
  });
}
export function useAdminResetMfa() {
  return useMutation({
    mutationFn: (id: number) => mfaApi.adminResetMfa(id),
    onSuccess: () => toast.success("MFA reset"),
    onError: (e) => toast.error("Could not reset MFA" + refSuffix(e)),
  });
}
