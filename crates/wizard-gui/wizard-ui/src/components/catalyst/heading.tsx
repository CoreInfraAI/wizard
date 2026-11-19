// SPDX-License-Identifier: LicenseRef-Tailwind-Plus
// Catalyst by Tailwind Labs Inc., with project modifications.
// See LICENSE.md in this directory. Excluded from the MPL-2.0 grant.

import clsx from "clsx";

type HeadingProps = {
  level?: 1 | 2 | 3 | 4 | 5 | 6;
} & React.ComponentPropsWithoutRef<"h1" | "h2" | "h3" | "h4" | "h5" | "h6">;

export function Heading({ className, level = 1, ...props }: HeadingProps) {
  let Element: `h${typeof level}` = `h${level}`;

  return (
    <Element
      {...props}
      className={clsx(
        className,
        "font-semibold text-zinc-950 dark:text-white",
        level === 1 ? "text-[22px]/8 tracking-tight" : level === 2 ? "text-base/6" : "text-sm/6",
      )}
    />
  );
}

export function Subheading({ className, level = 2, ...props }: HeadingProps) {
  let Element: `h${typeof level}` = `h${level}`;

  return (
    <Element
      {...props}
      className={clsx(
        className,
        "text-sm/6 font-semibold text-zinc-950 dark:text-white",
      )}
    />
  );
}
