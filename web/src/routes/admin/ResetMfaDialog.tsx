import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogTrigger, DialogClose } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useAdminResetMfa } from "@/api/hooks";

export function ResetMfaDialog({ userId }: { userId: number }) {
  const reset = useAdminResetMfa();
  return (
    <Dialog>
      <DialogTrigger render={<Button variant="outline" size="sm" />}>Reset MFA</DialogTrigger>
      <DialogContent>
        <DialogHeader><DialogTitle>Reset this user&apos;s MFA?</DialogTitle></DialogHeader>
        <p className="text-sm text-muted-foreground">
          Clears their second factor and recovery codes. Under a required policy they must re-enroll on next login.
        </p>
        <DialogFooter>
          <DialogClose render={<Button variant="ghost" />}>Cancel</DialogClose>
          <DialogClose render={<Button variant="destructive" onClick={() => reset.mutate(userId)} />}>Reset</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
