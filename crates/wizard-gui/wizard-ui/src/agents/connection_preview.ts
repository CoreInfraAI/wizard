import { createContext } from "react";

// Enabled only by mock entry points. Native Claude Desktop subscriptions still
// authenticate through the shared Wizard proxy, not an agent-specific token.
export const SubscriptionTokenPreviewContext = createContext(false);
