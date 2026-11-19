import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { AppSidebar } from "./AppSidebar";
import { AgentPage } from "./agents/AgentPage";
import { useAgentState } from "./agents/agents_state";
import { agentPageNames, pageFromHash } from "./agents/navigation";
import { useProxy } from "./agents/proxy";
import { SidebarLayout } from "./components/catalyst/sidebar-layout";
import { StateNotice } from "./components/ui/StateNotice";
import { PageHeader } from "./components/ui/Layout";
import { useHubAccountControl } from "./settings/HubAccount";
import { HubSignIn } from "./settings/HubSignIn";
import { SettingsPage } from "./settings/SettingsPage";
import { useUpdateState } from "./updates/update";
import { UpdateDialog } from "./updates/UpdateDialog";
import { Overview } from "./overview/Overview";
import { InstallationsProvider } from "./onboarding/Installations";
import { StartupScreen } from "./components/StartupScreen";

function subscribeNavigation(onChange: () => void) {
  window.addEventListener("hashchange", onChange);
  return () => window.removeEventListener("hashchange", onChange);
}

const startupKey = "coreinfra.startup.seen";

function needsStartup() {
  try {
    return (
      localStorage.getItem(startupKey) !== "true" &&
      localStorage.getItem("coreinfra.onboarding.completed") !== "true"
    );
  } catch {
    return true;
  }
}

function App() {
  const update = useUpdateState();
  const agents = useAgentState();
  const proxy = useProxy();
  const account = useHubAccountControl();
  const page = useSyncExternalStore(subscribeNavigation, () =>
    pageFromHash(window.location.hash),
  );
  const [updateOpen, setUpdateOpen] = useState(false);
  const [starting, setStarting] = useState(needsStartup);
  const finishStartup = useCallback(() => {
    try {
      localStorage.setItem(startupKey, "true");
    } catch {
      /* The current session can still continue. */
    }
    setStarting(false);
  }, []);

  // Pages share the window scroll; start each one from the top.
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [page]);

  if (starting) return <StartupScreen onComplete={finishStartup} />;

  return (
    <InstallationsProvider agents={agents} signedIn={account.signedIn}>
      <SidebarLayout
        fill={page === "overview"}
        navbar={null}
        sidebar={<AppSidebar page={page} account={account} update={update} onUpdate={() => setUpdateOpen(true)} />}
      >
        {page === "overview" ? (
          <Overview account={account} agents={agents} />
        ) : (
          <>
            <HubSignIn hub={account} compact />
            {page === "settings" && <SettingsPage />}
            {page !== "settings" && agents.status !== "ready" && (
              <div className="ui-stack">
                <PageHeader title={agentPageNames[page]} />
                <StateNotice
                  tone={agents.status === "loading" ? "loading" : "error"}
                >
                  {agents.status === "loading"
                    ? "Ищем установленных агентов…"
                    : `Не удалось загрузить состояние агента: ${agents.message}`}
                </StateNotice>
              </div>
            )}
            {agents.status === "ready" && (
              <AgentPage
                agents={agents.data}
                page={page === "settings" ? null : page}
                proxy={proxy}
                account={account}
              />
            )}
          </>
        )}
      </SidebarLayout>
      {updateOpen && <UpdateDialog observation={update} onClose={() => setUpdateOpen(false)} />}
    </InstallationsProvider>
  );
}

export default App;
