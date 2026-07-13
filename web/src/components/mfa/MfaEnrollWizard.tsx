import { useState } from "react";
import QRCode from "react-qr-code";
import { Button } from "@/components/ui/button";
import { MfaCodeInput } from "@/components/mfa/MfaCodeInput";

export function MfaEnrollWizard({
  provisioningUri, secret, onConfirm, pending,
}: { provisioningUri: string; secret: string; onConfirm: (code: string) => void; pending: boolean }) {
  const [code, setCode] = useState("");
  return (
    <div className="space-y-4">
      <p className="text-sm">Scan this with your authenticator app, then enter the 6-digit code.</p>
      <div className="rounded bg-white p-3 w-fit"><QRCode value={provisioningUri} size={160} /></div>
      <p className="text-xs text-muted-foreground">Or enter this key manually: <code className="select-all">{secret}</code></p>
      <MfaCodeInput value={code} onChange={setCode} onSubmit={() => onConfirm(code)} pending={pending} />
      <Button className="w-full" disabled={pending || code.length === 0} onClick={() => onConfirm(code)}>Confirm</Button>
    </div>
  );
}
