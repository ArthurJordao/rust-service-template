import { useState } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger, DialogFooter } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { useScopes, useSetUserScopes } from "@/api/hooks";
import type { UserWithScopes } from "@/api/types";

export function EditScopesDialog({ user }: { user: UserWithScopes }) {
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string[]>(user.scopes);
  const { data: catalog } = useScopes();
  const setScopes = useSetUserScopes();

  function toggle(name: string, on: boolean) {
    setSelected((s) => (on ? [...new Set([...s, name])] : s.filter((x) => x !== name)));
  }

  return (
    <Dialog open={open} onOpenChange={(o) => { setOpen(o); if (o) setSelected(user.scopes); }}>
      <DialogTrigger render={<Button variant="outline" size="sm" />}>Edit scopes</DialogTrigger>
      <DialogContent>
        <DialogHeader><DialogTitle>Scopes for {user.email}</DialogTitle></DialogHeader>
        <div className="space-y-2">
          {(catalog ?? []).map((s) => (
            <div key={s.id} className="flex items-center gap-2">
              <Checkbox id={`scope-${s.id}`} checked={selected.includes(s.name)} onCheckedChange={(v) => toggle(s.name, !!v)} />
              <Label htmlFor={`scope-${s.id}`} className="font-normal">{s.name}<span className="ml-2 text-xs text-muted-foreground">{s.description}</span></Label>
            </div>
          ))}
        </div>
        <DialogFooter>
          <Button
            disabled={setScopes.isPending}
            onClick={() => setScopes.mutate({ id: user.id, scopes: selected }, { onSuccess: () => setOpen(false) })}
          >Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
