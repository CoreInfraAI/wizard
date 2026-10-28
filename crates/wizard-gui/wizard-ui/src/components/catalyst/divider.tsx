// SPDX-License-Identifier: LicenseRef-Tailwind-Plus
// Catalyst by Tailwind Labs Inc., with project modifications.
// See LICENSE.md in this directory. Excluded from the MPL-2.0 grant.

import clsx from "clsx";

export function Divider({
  soft = false,
  className,
  ...props
}: { soft?: boolean } & React.ComponentPropsWithoutRef<"hr">) {
  return (
    <hr
      role="presentation"
      {...props}
      className={clsx(
        className,
        "w-full border-t",
        soft && "border-zinc-950/5 dark:border-white/5",
        !soft && "border-zinc-950/10 dark:border-white/10",
      )}
    />
  );
}
