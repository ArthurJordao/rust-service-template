import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { useAuth } from "@/auth/AuthProvider";
import { useMe, useMfaStatus, useDisableMfa, useRegenRecoveryCodes } from "@/api/hooks";
import { mfaSetup, mfaConfirm } from "@/api/mfa";
import { refSuffix } from "@/lib/errors";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { MfaEnrollWizard } from "@/components/mfa/MfaEnrollWizard";
import { RecoveryCodesDialog } from "@/components/mfa/RecoveryCodesDialog";

function EnableMfaButton() {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [setup, setSetup] = useState<{ provisioning_uri: string; secret: string } | null>(null);
  const [pending, setPending] = useState(false);
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);

  async function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) return;
    setSetup(null);
    try {
      setSetup(await mfaSetup());
    } catch (e) {
      toast.error("Couldn't start MFA setup" + refSuffix(e));
      setOpen(false);
    }
  }

  async function handleConfirm(code: string) {
    setPending(true);
    try {
      const res = await mfaConfirm(code);
      setOpen(false);
      setRecoveryCodes(res.recovery_codes);
    } catch (e) {
      toast.error("Enrollment failed" + refSuffix(e));
    } finally {
      setPending(false);
    }
  }

  function handleDone() {
    setRecoveryCodes(null);
    setSetup(null);
    qc.invalidateQueries({ queryKey: ["mfa-status"] });
  }

  return (
    <>
      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogTrigger render={<Button size="sm" />}>Enable MFA</DialogTrigger>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Set up two-factor authentication</DialogTitle>
          </DialogHeader>
          {setup ? (
            <MfaEnrollWizard
              provisioningUri={setup.provisioning_uri}
              secret={setup.secret}
              onConfirm={handleConfirm}
              pending={pending}
            />
          ) : (
            <p className="text-sm text-muted-foreground">Setting up…</p>
          )}
        </DialogContent>
      </Dialog>
      {recoveryCodes && <RecoveryCodesDialog codes={recoveryCodes} open onDone={handleDone} />}
    </>
  );
}

function RegenerateButton() {
  const regen = useRegenRecoveryCodes();
  const [codes, setCodes] = useState<string[] | null>(null);

  function handleClick() {
    if (!window.confirm("Regenerate recovery codes? Your existing codes will stop working.")) return;
    regen.mutate(undefined, { onSuccess: (data) => setCodes(data) });
  }

  return (
    <>
      <Button variant="outline" size="sm" disabled={regen.isPending} onClick={handleClick}>
        Regenerate recovery codes
      </Button>
      {codes && <RecoveryCodesDialog codes={codes} open onDone={() => setCodes(null)} />}
    </>
  );
}

function DisableButton() {
  const disable = useDisableMfa();

  function handleClick() {
    if (!window.confirm("Disable two-factor authentication?")) return;
    disable.mutate();
  }

  return (
    <Button variant="destructive" size="sm" disabled={disable.isPending} onClick={handleClick}>
      Disable MFA
    </Button>
  );
}

export function AccountPage() {
  const { user } = useAuth();
  const { data, isLoading, error } = useMe(!!user);
  const { data: mfa } = useMfaStatus(!!user);

  return (
    <div className="max-w-md">
      <Card className="p-6">
        <h1 className="mb-4 text-xl font-semibold">My account</h1>
        {isLoading && <p>Loading…</p>}
        {error && (
          <p className="text-sm text-muted-foreground">
            No account yet for {user?.email}.
          </p>
        )}
        {data && (
          <dl className="space-y-2 text-sm">
            <div>
              <dt className="text-muted-foreground">Email</dt>
              <dd>{data.email}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Account ID</dt>
              <dd>{data.id}</dd>
            </div>
            <div>
              <dt className="text-muted-foreground">Created</dt>
              <dd>{new Date(data.created_at).toLocaleString()}</dd>
            </div>
          </dl>
        )}
      </Card>

      {mfa && mfa.policy !== "off" && (
        <Card className="p-6 mt-6">
          <h2 className="text-lg font-semibold mb-2">Two-factor authentication</h2>
          {mfa.enabled ? (
            <div className="space-y-2">
              <p className="text-sm text-muted-foreground">Two-factor authentication is on.</p>
              <div className="flex gap-2">
                <RegenerateButton />
                {mfa.policy === "optional" && <DisableButton />}
              </div>
            </div>
          ) : (
            <EnableMfaButton />
          )}
        </Card>
      )}
    </div>
  );
}
