import { useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { m } from "@/paraglide/messages.js";

type AccountDeleteDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  accountLabel: string;
};

function AccountDeleteDialog({
  open,
  onOpenChange,
  onConfirm,
  accountLabel,
}: AccountDeleteDialogProps) {
  const description = open ? m.providers_account_delete_description({ label: accountLabel }) : "";
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent data-slot="account-delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>{m.providers_account_delete_title()}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
          <AlertDialogAction
            onClick={onConfirm}
            className="bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60"
          >
            {m.common_delete()}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

type AccountDeleteActionProps = {
  accountLabel: string;
  buttonLabel: string;
  disabled: boolean;
  onConfirm: () => void;
};

export function AccountDeleteAction({
  accountLabel,
  buttonLabel,
  disabled,
  onConfirm,
}: AccountDeleteActionProps) {
  const [open, setOpen] = useState(false);

  const handleConfirm = () => {
    setOpen(false);
    onConfirm();
  };

  return (
    <div data-slot="account-delete-action" className="contents">
      <Button
        type="button"
        variant="ghost"
        size="sm"
        onClick={() => setOpen(true)}
        disabled={disabled}
      >
        {buttonLabel}
      </Button>
      <AccountDeleteDialog
        open={open}
        onOpenChange={setOpen}
        onConfirm={handleConfirm}
        accountLabel={accountLabel}
      />
    </div>
  );
}
