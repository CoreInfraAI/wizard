import type { ReactNode } from "react";
import { Text } from "./catalyst/text";

export function ErrorText({ children }: { children: ReactNode }) {
  return <Text className="text-red-600! dark:text-red-400!">{children}</Text>;
}
