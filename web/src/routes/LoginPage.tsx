import React, { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { useAuth, type MfaChallenge } from "@/auth/AuthProvider";
import { refSuffix } from "@/lib/errors";
import * as mfa from "@/api/mfa";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card } from "@/components/ui/card";
import { MfaCodeInput } from "@/components/mfa/MfaCodeInput";
import { MfaEnrollWizard } from "@/components/mfa/MfaEnrollWizard";
import { RecoveryCodesDialog } from "@/components/mfa/RecoveryCodesDialog";
import type { components } from "@/api/schema";
type AccessTokenResponse = components["schemas"]["AccessTokenResponse"];

export function LoginPage() {
  const { login, applySession } = useAuth();
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [challenge, setChallenge] = useState<MfaChallenge | null>(null);
  const [setup, setSetup] = useState<{ provisioning_uri: string; secret: string } | null>(null);
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState<{ codes: string[]; tokens: AccessTokenResponse } | null>(null);

  function resetToPassword(msg: string, e?: unknown) {
    toast.error(msg + refSuffix(e));
    setChallenge(null); setSetup(null); setCode("");
  }

  async function onPassword(e: React.FormEvent) {
    e.preventDefault(); setBusy(true);
    let ch: MfaChallenge | null;
    try {
      ch = await login(email, password);
    } catch (e) {
      toast.error("Invalid credentials" + refSuffix(e));
      setBusy(false);
      return;
    }
    if (!ch) { navigate("/"); setBusy(false); return; }
    setChallenge(ch);
    if (ch.purpose === "enroll") {
      try {
        const s = await mfa.mfaSetup(ch.mfa_token);
        setSetup(s);
      } catch (e) {
        resetToPassword("Couldn't start MFA setup — please sign in again", e);
      }
    }
    setBusy(false);
  }

  async function onVerify() {
    if (!challenge || busy) return; setBusy(true);
    try {
      const tokens = await mfa.mfaVerify(code, challenge.mfa_token);
      applySession(tokens); navigate("/");
    } catch (e) { resetToPassword("Code rejected — please sign in again", e); }
    finally { setBusy(false); }
  }

  async function onConfirmEnroll(c: string) {
    if (!challenge || busy) return; setBusy(true);
    try {
      const res = await mfa.mfaConfirm(c, challenge.mfa_token);
      if (res.tokens) setRecovery({ codes: res.recovery_codes, tokens: res.tokens });
      else resetToPassword("Enrollment succeeded but no session was returned — please sign in again");
    } catch (e) { resetToPassword("Enrollment failed — please sign in again", e); }
    finally { setBusy(false); }
  }

  return (
    <div className="mx-auto mt-24 max-w-sm">
      <Card className="p-6">
        {!challenge && (<>
          <h1 className="mb-4 text-xl font-semibold">Sign in</h1>
          <form onSubmit={onPassword} className="space-y-4">
            <div className="space-y-1"><Label htmlFor="email">Email</Label>
              <Input id="email" type="email" value={email} onChange={(e) => setEmail(e.target.value)} required /></div>
            <div className="space-y-1"><Label htmlFor="password">Password</Label>
              <Input id="password" type="password" value={password} onChange={(e) => setPassword(e.target.value)} required /></div>
            <Button type="submit" className="w-full" disabled={busy}>Sign in</Button>
          </form>
          <p className="mt-4 text-sm">No account? <Link to="/register" className="underline">Register</Link></p>
        </>)}

        {challenge?.purpose === "verify" && (<>
          <h1 className="mb-4 text-xl font-semibold">Two-factor authentication</h1>
          <div className="space-y-4">
            <MfaCodeInput value={code} onChange={setCode} onSubmit={onVerify} pending={busy} />
            <Button className="w-full" disabled={busy || !code} onClick={onVerify}>Verify</Button>
          </div>
        </>)}

        {challenge?.purpose === "enroll" && !setup && !recovery && (
          <p className="text-sm text-muted-foreground">Setting up two-factor authentication…</p>
        )}

        {challenge?.purpose === "enroll" && setup && !recovery && (<>
          <h1 className="mb-4 text-xl font-semibold">Set up two-factor authentication</h1>
          <MfaEnrollWizard provisioningUri={setup.provisioning_uri} secret={setup.secret} onConfirm={onConfirmEnroll} pending={busy} />
        </>)}

        {recovery && (
          <RecoveryCodesDialog codes={recovery.codes} open onDone={() => { applySession(recovery.tokens); navigate("/"); }} />
        )}
      </Card>
    </div>
  );
}
