// SPDX-License-Identifier: LicenseRef-Tailwind-Plus
// Catalyst by Tailwind Labs Inc., with project modifications.
// See LICENSE.md in this directory. Excluded from the MPL-2.0 grant.

import * as Headless from "@headlessui/react";
import { forwardRef } from "react";
import type * as React from "react";

export const Link = forwardRef(function Link(
  props: { href: string } & React.ComponentPropsWithoutRef<"a">,
  ref: React.ForwardedRef<HTMLAnchorElement>,
) {
  return (
    <Headless.DataInteractive>
      <a {...props} ref={ref} />
    </Headless.DataInteractive>
  );
});
