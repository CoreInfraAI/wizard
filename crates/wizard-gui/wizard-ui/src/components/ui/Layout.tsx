import { useId, type ComponentPropsWithoutRef, type ReactNode } from "react";
import clsx from "clsx";
import { Heading } from "../catalyst/heading";
import { Text } from "../catalyst/text";

export function PageHeader({
  title,
  description,
  actions,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="ui-page-header">
      <div className="ui-page-header-copy">
        <Heading>{title}</Heading>
        {description && <Text>{description}</Text>}
      </div>
      {actions && <div className="ui-page-actions">{actions}</div>}
    </header>
  );
}

export function Panel({
  title,
  description,
  tone = "default",
  framed = true,
  children,
  className,
  ...props
}: {
  title?: string;
  description?: ReactNode;
  tone?: "default" | "muted";
  framed?: boolean;
} & Omit<ComponentPropsWithoutRef<"section">, "title">) {
  const id = useId();
  return (
    <section
      {...props}
      className={clsx("ui-panel", className)}
      data-tone={tone}
      data-framed={framed}
      aria-labelledby={title ? id : undefined}
    >
      {title && (
        <header className="ui-panel-heading">
          <Heading level={2} id={id}>
            {title}
          </Heading>
          {description && <Text>{description}</Text>}
        </header>
      )}
      {children}
    </section>
  );
}

export function ActionBar({
  children,
  leading,
  divider = false,
  className,
}: {
  children: ReactNode;
  leading?: ReactNode;
  divider?: boolean;
  className?: string;
}) {
  return (
    <div
      className={clsx("ui-actions", className)}
      data-separated={Boolean(leading)}
      data-divider={divider}
    >
      {leading && <div className="ui-actions-group">{leading}</div>}
      <div className="ui-actions-group">{children}</div>
    </div>
  );
}

/** Key only presentation; ongoing operations belong to their page/provider. */
export function ViewTransition({
  children,
  view,
  className,
}: {
  children: ReactNode;
  view: string;
  className?: string;
}) {
  return (
    <div key={view} className={clsx("ui-route-enter", className)}>
      {children}
    </div>
  );
}
