import type { ReactNode } from "react";
import { XMarkIcon } from "@heroicons/react/20/solid";
import { Button } from "../catalyst/button";
import { DialogDescription, DialogTitle } from "../catalyst/dialog";

export function DialogHeader({
  title,
  description,
  onClose,
  closeLabel = "Закрыть",
  disabled = false,
}: {
  title: ReactNode;
  description?: ReactNode;
  onClose?: () => void;
  closeLabel?: string;
  disabled?: boolean;
}) {
  return (
    <div className="ui-dialog-heading">
      <div className="ui-dialog-heading-copy">
        <DialogTitle tabIndex={-1}>{title}</DialogTitle>
        {description && <DialogDescription>{description}</DialogDescription>}
      </div>
      {onClose && (
        <Button
          plain
          size="sm"
          className="ui-dialog-close"
          aria-label={closeLabel}
          title={closeLabel}
          disabled={disabled}
          onClick={onClose}
        >
          <XMarkIcon />
        </Button>
      )}
    </div>
  );
}
