/**
 * TODO: Update this component to use your client-side framework's link
 * component. We've provided examples of how to do this for Next.js, Remix, and
 * Inertia.js in the Catalyst documentation:
 *
 * https://catalyst.tailwindui.com/docs#client-side-router-integration
 */

import * as Headless from "@headlessui/react";
import { forwardRef } from "react";
import type * as React from "react";
import { Link as WouterLink } from "wouter";

export const Link = forwardRef(function Link(
  props: { href: string } & React.ComponentPropsWithoutRef<"a">,
  ref: React.ForwardedRef<HTMLAnchorElement>,
) {
  const isExternal =
    props.href.startsWith("http://") ||
    props.href.startsWith("https://") ||
    props.href.startsWith("mailto:") ||
    props.href.startsWith("tel:");

  let { href, onClick, ...rest } = props;

  return (
    <Headless.DataInteractive>
      {isExternal ? (
        <a href={href} onClick={onClick} {...rest} ref={ref} />
      ) : (
        <WouterLink
          href={href}
          {...(onClick === undefined ? {} : { onClick })}
          asChild
        >
          <a href={href} {...rest} ref={ref} />
        </WouterLink>
      )}
    </Headless.DataInteractive>
  );
});
