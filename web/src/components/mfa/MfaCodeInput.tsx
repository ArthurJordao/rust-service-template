import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export function MfaCodeInput({
  value, onChange, onSubmit, pending,
}: { value: string; onChange: (v: string) => void; onSubmit: () => void; pending: boolean }) {
  return (
    <div className="space-y-1">
      <Label htmlFor="mfa-code">Authentication or recovery code</Label>
      <Input
        id="mfa-code"
        autoFocus
        autoComplete="one-time-code"
        value={value}
        disabled={pending}
        onChange={(e) => onChange(e.target.value.trim())}
        onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); onSubmit(); } }}
      />
    </div>
  );
}
