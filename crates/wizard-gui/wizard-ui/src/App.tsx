import { useEffect, useState } from "react";
import { AppSidebar } from "./AppSidebar";
import { AgentPage } from "./agents/AgentPage";
import { type AgentId, useAgentState } from "./agents/agents_state";
import { useProxy } from "./agents/proxy";
import { AuthLayout } from "./components/catalyst/auth-layout";
import { Badge } from "./components/catalyst/badge";
import { SidebarLayout } from "./components/catalyst/sidebar-layout";
import { ErrorText } from "./components/ErrorText";
import { OpenLogsButton } from "./logs/OpenLogsButton";
import { useHubAccountControl } from "./settings/HubAccount";
import { useUpdateState } from "./updates/update";
import { UpdateScreen } from "./updates/UpdateScreen";

function App() {
  const update = useUpdateState();
  const agents = useAgentState();
  const proxy = useProxy();
  const account = useHubAccountControl();
  const [page, setPage] = useState<AgentId>("claude_desktop");
  const state = update.state;

  // Pages share the window scroll; start each one from the top.
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [page]);

  if (update.error === undefined && (!state || state.status === "checking" || state.status === "installing")) {
    return (
      <AuthLayout>
        <div className="flex flex-col items-center gap-8">
          <UpdateScreen state={state} />
          <OpenLogsButton />
        </div>
      </AuthLayout>
    );
  }

  return (
    <SidebarLayout
      navbar={null}
      sidebar={<AppSidebar page={page} onSelect={setPage} agents={agents.status === "ready" ? agents.data : undefined}
        proxy={proxy} account={account} update={update} />}
    >
      {agents.status === "loading" && <Badge color="sky" className="animate-pulse">Загрузка…</Badge>}
      {agents.status === "error" && <ErrorText>Не удалось загрузить состояние агентов: {agents.message}</ErrorText>}
      {agents.status === "ready" && <AgentPage agents={agents.data} page={page} proxy={proxy} account={account} />}
    </SidebarLayout>
  );
}

export default App;
