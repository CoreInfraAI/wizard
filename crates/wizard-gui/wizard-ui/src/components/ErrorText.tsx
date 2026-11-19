import type { ReactNode } from "react";
import { Text } from "./catalyst/text";

export function ErrorText({ children }: { children: ReactNode }) {
  return <Text className="ui-error">{children}</Text>;
}
