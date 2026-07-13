import { useState } from "react";
import { toast } from "sonner";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

export function RecoveryCodesDialog({ codes, open, onDone }: { codes: string[]; open: boolean; onDone: () => void }) {
  const [ack, setAck] = useState(false);
  const text = codes.join("\n");

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      toast.error("Couldn't copy — copy manually");
    }
  }

  return (
    <Dialog open={open}>
      <DialogContent showCloseButton={false}>
        <DialogHeader><DialogTitle>Save your recovery codes</DialogTitle></DialogHeader>
        <p className="text-sm text-muted-foreground">
          Each code works once. Store them somewhere safe — they won't be shown again.
        </p>
        <pre className="grid grid-cols-2 gap-1 rounded bg-muted p-3 text-sm">{codes.map((c) => <span key={c}>{c}</span>)}</pre>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={handleCopy}>Copy</Button>
          <Button variant="outline" size="sm" onClick={() => {
            const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
            const a = document.createElement("a"); a.href = url; a.download = "recovery-codes.txt"; a.click();
            URL.revokeObjectURL(url);
          }}>Download</Button>
        </div>
        <label className="flex items-center gap-2 text-sm">
          <Checkbox checked={ack} onCheckedChange={(v) => setAck(v)} /> I've saved these codes
        </label>
        <DialogFooter>
          <Button disabled={!ack} onClick={onDone}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
