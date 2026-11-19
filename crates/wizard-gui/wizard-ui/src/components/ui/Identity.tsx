import type { ReactNode } from "react";
import { Heading } from "../catalyst/heading";
import { Text } from "../catalyst/text";

export function Identity({
  icon,
  title,
  description,
  focusable = false,
}: {
  icon: ReactNode;
  title: string;
  description?: ReactNode;
  focusable?: boolean;
}) {
  return (
    <div className="ui-identity">
      <span className="ui-identity-icon" aria-hidden="true">
        {icon}
      </span>
      <div className="ui-identity-copy">
        <Heading level={2} tabIndex={focusable ? -1 : undefined}>
          {title}
        </Heading>
        {description && <Text>{description}</Text>}
      </div>
    </div>
  );
}
