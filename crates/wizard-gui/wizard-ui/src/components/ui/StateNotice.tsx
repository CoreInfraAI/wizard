import type { ComponentType, ReactNode, SVGProps } from "react";
import {
  ArrowPathIcon,
  CheckCircleIcon,
  ExclamationCircleIcon,
  InformationCircleIcon,
} from "@heroicons/react/20/solid";
import { Heading } from "../catalyst/heading";
import { Text } from "../catalyst/text";

export function StateNotice({
  tone = "info",
  children,
  actions,
}: {
  tone?: "info" | "loading" | "error" | "warning" | "success";
  children: ReactNode;
  actions?: ReactNode;
}) {
  const Icon =
    tone === "loading"
      ? ArrowPathIcon
      : tone === "success"
        ? CheckCircleIcon
        : tone === "error" || tone === "warning"
          ? ExclamationCircleIcon
          : InformationCircleIcon;
  return (
    <div
      className="ui-state"
      data-tone={tone}
      role={tone === "error" ? "alert" : "status"}
    >
      <Icon
        aria-hidden="true"
        className={tone === "loading" ? "ui-spin" : undefined}
      />
      <div className="ui-state-copy">
        {children}
        {actions && <div className="ui-state-actions">{actions}</div>}
      </div>
    </div>
  );
}

export function EmptyState({
  title,
  headingLevel = 3,
  children,
  icon: Icon,
  action,
}: {
  title: string;
  headingLevel?: 2 | 3;
  children?: ReactNode;
  icon?: ComponentType<SVGProps<SVGSVGElement>>;
  action?: ReactNode;
}) {
  return (
    <div className="ui-empty">
      {Icon && <Icon aria-hidden="true" />}
      <Heading level={headingLevel}>{title}</Heading>
      {children && <Text>{children}</Text>}
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}
