// SPDX-License-Identifier: LicenseRef-Tailwind-Plus
// Catalyst by Tailwind Labs Inc., with project modifications.
// See LICENSE.md in this directory. Excluded from the MPL-2.0 grant.

"use client";

import * as Headless from "@headlessui/react";
import clsx from "clsx";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import type * as React from "react";
import { NavbarItem } from "./navbar.js";

const SidebarActionContext = createContext<(action: () => void) => void>(
  (action) => action(),
);

/** Open overlays only after mobile navigation has released its focus trap. */
export function useSidebarAction() {
  return useContext(SidebarActionContext);
}

function OpenMenuIcon() {
  return (
    <svg data-slot="icon" viewBox="0 0 20 20" aria-hidden="true">
      <path d="M2 6.75C2 6.33579 2.33579 6 2.75 6H17.25C17.6642 6 18 6.33579 18 6.75C18 7.16421 17.6642 7.5 17.25 7.5H2.75C2.33579 7.5 2 7.16421 2 6.75ZM2 13.25C2 12.8358 2.33579 12.5 2.75 12.5H17.25C17.6642 12.5 18 12.8358 18 13.25C18 13.6642 17.6642 14 17.25 14H2.75C2.33579 14 2 13.6642 2 13.25Z" />
    </svg>
  );
}

function CloseMenuIcon() {
  return (
    <svg data-slot="icon" viewBox="0 0 20 20" aria-hidden="true">
      <path d="M6.28 5.22a.75.75 0 0 0-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 1 0 1.06 1.06L10 11.06l3.72 3.72a.75.75 0 1 0 1.06-1.06L11.06 10l3.72-3.72a.75.75 0 0 0-1.06-1.06L10 8.94 6.28 5.22Z" />
    </svg>
  );
}

function MobileSidebar({
  open,
  close,
  afterLeave,
  children,
}: React.PropsWithChildren<{
  open: boolean;
  close: () => void;
  afterLeave: () => void;
}>) {
  return (
    <Headless.Transition show={open} afterLeave={afterLeave}>
      <Headless.Dialog
        onClose={close}
        className="lg:hidden"
        aria-label="Навигация"
      >
        <Headless.DialogBackdrop
          transition
          className="fixed inset-0 bg-black/30 transition motion-reduce:transition-none data-closed:opacity-0 data-enter:duration-300 data-enter:ease-out data-leave:duration-200 data-leave:ease-in"
        />
        <Headless.DialogPanel
          transition
          className="fixed inset-y-0 w-full max-w-80 p-2 mac-window:pt-8 transition duration-300 motion-reduce:transition-none ease-in-out data-closed:-translate-x-full"
        >
          <div className="flex h-full flex-col rounded-lg bg-white shadow-xs ring-1 ring-zinc-950/5 dark:bg-zinc-900 dark:ring-white/10">
            <div className="-mb-3 px-4 pt-3">
              <Headless.CloseButton as={NavbarItem} aria-label="Закрыть меню">
                <CloseMenuIcon />
              </Headless.CloseButton>
            </div>
            {children}
          </div>
        </Headless.DialogPanel>
      </Headless.Dialog>
    </Headless.Transition>
  );
}

export function SidebarLayout({
  navbar,
  sidebar,
  fill = false,
  hideNavigation = false,
  children,
}: React.PropsWithChildren<{
  navbar: React.ReactNode;
  sidebar: React.ReactNode;
  /** Fill the viewport with unpadded content; children own their scrolling. */
  fill?: boolean;
  /** Keep first-run setup focused, without desktop or mobile navigation. */
  hideNavigation?: boolean;
}>) {
  let [showSidebar, setShowSidebar] = useState(false);
  const pendingAction = useRef<(() => void) | undefined>(undefined);
  const actionFrame = useRef<number | undefined>(undefined);
  useEffect(
    () => () => {
      if (actionFrame.current !== undefined)
        cancelAnimationFrame(actionFrame.current);
    },
    [],
  );

  function runAfterClose(action: () => void) {
    pendingAction.current = action;
    setShowSidebar(false);
  }

  function finishClosing() {
    const action = pendingAction.current;
    pendingAction.current = undefined;
    // Let the previous dialog unmount and restore focus before mounting another.
    if (action) actionFrame.current = requestAnimationFrame(action);
  }

  return (
    <div
      className={clsx(
        "relative isolate flex w-full bg-white max-lg:flex-col dark:bg-zinc-900",
        !hideNavigation && "lg:bg-zinc-100 dark:lg:bg-zinc-950",
        fill ? "h-dvh overflow-hidden" : "min-h-svh",
      )}
    >
      {/* Sidebar on desktop */}
      {!hideNavigation && (
        <div className="fixed inset-y-0 left-0 w-64 max-lg:hidden">
          {sidebar}
        </div>
      )}

      {/* Sidebar on mobile */}
      {!hideNavigation && (
        <MobileSidebar
          open={showSidebar}
          close={() => setShowSidebar(false)}
          afterLeave={finishClosing}
        >
          <SidebarActionContext.Provider value={runAfterClose}>
            {sidebar}
          </SidebarActionContext.Provider>
        </MobileSidebar>
      )}

      {/* Navbar on mobile */}
      {!hideNavigation && (
        <header
          className={clsx(
            "sticky top-0 z-20 flex items-center gap-3 bg-white px-4 dark:bg-zinc-900 mac-window:pt-8 lg:hidden",
            fill && "shrink-0",
          )}
        >
          <div className="py-2.5">
            <NavbarItem
              onClick={() => setShowSidebar(true)}
              aria-label="Открыть меню"
            >
              <OpenMenuIcon />
            </NavbarItem>
          </div>
          <div className="min-w-0 flex-1">{navbar}</div>
        </header>
      )}

      {/* Content */}
      <main
        className={clsx(
          "flex flex-1 flex-col lg:min-w-0",
          !hideNavigation && "lg:pt-2 lg:pr-2 lg:pl-64",
          fill ? "min-h-0 min-w-0" : "pb-2",
          fill && !hideNavigation && "lg:pb-2",
        )}
      >
        <div
          className={clsx(
            !hideNavigation &&
              "lg:rounded-lg lg:bg-white lg:shadow-xs lg:ring-1 lg:ring-zinc-950/5 dark:lg:bg-zinc-900 dark:lg:ring-white/10",
            fill
              ? "flex min-h-0 flex-1 flex-col overflow-hidden"
              : "grow p-(--ui-page-gutter)",
          )}
        >
          {fill ? children : <div className="ui-page">{children}</div>}
        </div>
      </main>
    </div>
  );
}
