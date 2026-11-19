import { useEffect, useRef, useState } from "react";
import { ArrowUpRightIcon } from "@heroicons/react/20/solid";
import { hasAgentDetectionError, type useAgentState } from "../agents/agents_state";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Text, TextLink } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { StateNotice } from "../components/ui/StateNotice";
import { Dialog } from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import type { HubAccountControl } from "../settings/HubAccount";
import { hubDisplayName } from "../settings/hub_account";
import { formatHubBalance, needsHubTopUp, useHubSummary } from "../settings/hub_summary";
import { AgentIcon, choices, isFound } from "../onboarding/agents";
import { useInstallations } from "../onboarding/installations_state";
import { SetupDialog } from "../onboarding/SetupDialog";
import { OverviewArtwork } from "./OverviewArtwork";
import { OverviewBackdrop } from "./OverviewBackdrop";
import { OverviewCard, OverviewCardContent } from "./OverviewCard";
import { OverviewVideo } from "./OverviewVideo";
import { OverviewAgents } from "./OverviewAgents";
import { useOverviewTransition } from "./useOverviewTransition";
import "./Overview.css";

export function Overview({
  account,
  agents,
}: {
  account: HubAccountControl;
  agents: ReturnType<typeof useAgentState>;
}) {
  const [setup, setSetup] = useState<"select">();
  const [management, setManagement] = useState(false);
  const overviewRoot = useRef<HTMLDivElement>(null);
  const [registrationOpened, setRegistrationOpened] = useState(false);
  const connected = account.signedIn;
  const pending = account.account?.pending;
  const loadingAccount = !account.account;
  const installed =
    agents.status === "ready"
      ? choices.filter((choice) => isFound(choice, agents.data))
      : [];
  const canConfigure = connected && installed.length > 0;
  const activeHub = account.account?.profile?.hubs.find(
    (hub) => hub.hub_id === account.account?.active_hub_id,
  );
  const hubName = activeHub?.is_personal
    ? "Личный хаб"
    : activeHub
      ? hubDisplayName(activeHub)
      : "CoreInfra AI Hub";
  const profile = account.account?.profile;
  const accountDisabled =
    account.submitting || (!connected && (loadingAccount || account.agentBusy));
  const accountAction = connected ? account.open : account.change;
  const installations = useInstallations();
  const openInstall = () => { setManagement(false); installations.open(); };
  const openConfigure = () => { setManagement(false); setSetup("select"); };
  const detectionError =
    agents.status === "error" ||
    (agents.status === "ready" &&
      choices.some((choice) => hasAgentDetectionError(agents.data[choice.agent])));
  const configured = agents.status === "ready" && Object.values(agents.data).some(
    (agent) => typeof agent === "object" && agent.status === "found" && agent.data.routing_mode !== "disabled",
  );
  const connectedAgents = agents.status === "ready" ? installed.filter(choice => {
    const agent = agents.data[choice.agent];
    return agent.status === "found" && agent.data.routing_mode !== "disabled";
  }) : [];
  const listedAgents = connectedAgents.length ? connectedAgents : installed;
  const { dashboard: showDashboard, merging } = useOverviewTransition(overviewRoot, {
    ready: !loadingAccount && agents.status !== "loading",
    connected,
    requested: connected && (configured || detectionError),
    suspended: !!setup,
  });
  const summary = useHubSummary(account.account?.active_hub_id, connected);
  const guest = summary.data?.role_in_hub === "guest";
  const balance = summary.data?.balance_rub;
  const balanceLabel = summary.error ??
    (summary.loading ? "Загружаем баланс…" : typeof balance === "number" && Number.isFinite(balance)
      ? formatHubBalance(balance) : "Баланс недоступен");

  useEffect(() => {
    if (!connected) { setSetup(undefined); setManagement(false); }
  }, [connected]);

  // Wait for the first snapshot before deciding between onboarding and daily use.
  if (loadingAccount || agents.status === "loading") return (
    <div className="overview"><div className="overview-content ui-stack">
      <Heading>Обзор</Heading>
      <StateNotice tone={account.error ? "error" : "loading"}>
        {account.error ? "Не удалось проверить аккаунт. Повторяем подключение автоматически." : "Проверяем аккаунт и установленные приложения…"}
      </StateNotice>
    </div></div>
  );

  return (
    <div className="overview">
      <div className="overview-content" ref={overviewRoot} data-merging={merging}>
        <header className="overview-hero">
          <OverviewBackdrop />
          <div className="overview-hero-copy">
            <Heading><span className="overview-hero-text">Ура! Wizard установлен.</span></Heading>
            <Text className="mt-2"><span className="overview-hero-text">
                {connected
                  ? detectionError
                    ? "Проверяем установленные приложения."
                    : installed.length
                    ? "Выберите агента и настройте подключение к хабу."
                    : "Установите приложение и подключите его к хабу."
                  : "Осталось подключить аккаунт и настроить агентов."}
            </span></Text>
          </div>
          <OverviewVideo />
        </header>
        {showDashboard && configured && needsHubTopUp(summary.data) && (
          <div className="overview-ready-notice" role="status">
            <div>
              <Heading level={2}>Агент готов!</Heading>
              <Text>Чтобы начать использовать – пополните баланс хаба.</Text>
            </div>
            <Button disabled={account.submitting} onClick={account.topUp}>Пополнить</Button>
          </div>
        )}
        <section
          aria-labelledby={showDashboard ? undefined : "overview-actions-title"}
          aria-label={showDashboard ? "Хаб и агенты" : undefined}
          className="overview-actions"
        >
          {!showDashboard && <div className="overview-actions-header">
            <Heading level={2} id="overview-actions-title">
              Начало работы
            </Heading>
          </div>}
          <div className="overview-cards" data-dashboard={showDashboard}>
            <OverviewCard
              key="account"
              kind="account"
              onboarding={!showDashboard}
              disabled={accountDisabled}
              suspended={!!setup}
            >
              <OverviewArtwork
                kind="account"
                step={!showDashboard}
                connected={connected}
                disabled={accountDisabled}
                onClick={accountAction}
                label={
                  connected
                    ? `Открыть хаб: ${hubName}`
                    : "Подключить аккаунт CoreInfra Hub"
                }
              />
              <OverviewCardContent
                title={connected ? hubName : "Подключите свой аккаунт"}
                headingLevel={showDashboard ? 2 : 3}
                view={showDashboard ? "dashboard" : connected ? "connected" : "login"}
                descriptionTitle={
                  connected
                    ? (profile?.verified_email ?? profile?.username)
                    : undefined
                }
                description={
                  showDashboard ? <>
                    <span className="overview-hub-identity">{profile?.verified_email ?? profile?.username}</span>
                    {!guest && <span className="overview-balance">
                      <span className="overview-balance-label">Баланс хаба</span>
                      <strong className="overview-balance-value" data-amount={typeof balance === "number" && Number.isFinite(balance)}>{balanceLabel}</strong>
                    </span>}
                  </> : connected
                    ? (profile?.verified_email ??
                      profile?.username ??
                      "CoreInfra Hub")
                    : pending
                      ? "Завершите вход в браузере. Wizard подключится автоматически."
                      : registrationOpened
                        ? "Создайте аккаунт в браузере, затем вернитесь и войдите в хаб."
                        : "Войдите в CoreInfra Hub, чтобы подключить своих агентов."
                }
                actions={
                  <>
                    {connected ? (
                      <Button
                        outline
                        disabled={accountDisabled}
                        onClick={accountAction}
                      >
                        {showDashboard ? "Перейти в хаб" : "Открыть хаб"}
                        <ArrowUpRightIcon />
                      </Button>
                    ) : (
                      <Button
                        disabled={accountDisabled}
                        onClick={accountAction}
                      >
                        {loadingAccount
                          ? "Проверяем аккаунт…"
                          : account.submitting
                            ? "Открываем браузер…"
                            : pending
                              ? "Продолжить вход"
                              : "Войти"}
                      </Button>
                    )}
                    {!connected && !pending && (
                      <TextLink
                        href="https://hub.coreinfra.ai/register?source=wizard"
                        aria-disabled={loadingAccount || account.submitting}
                        onClick={(event) => {
                          event.preventDefault();
                          if (!loadingAccount && !account.submitting) {
                            setRegistrationOpened(true);
                            account.register();
                          }
                        }}
                      >
                        Создать аккаунт
                      </TextLink>
                    )}
                  </>
                }
              />
            </OverviewCard>

            {!showDashboard && <OverviewCard
              key="install"
              kind="install"
              disabled={!connected}
              activationReady={!loadingAccount}
              suspended={!!setup}
            >
              <OverviewArtwork
                kind="install"
                disabled={!connected}
                onClick={openInstall}
                label="Перейти к установке агентов"
                describedBy="overview-install-help"
              />
              <OverviewCardContent
                title="Установите агентов"
                description="Скачайте Claude Desktop или ChatGPT прямо из Wizard."
                descriptionId="overview-install-help"
                actions={
                  <Button
                    outline
                    disabled={!connected}
                    aria-describedby="overview-install-help"
                    onClick={openInstall}
                  >
                    Установить агента
                  </Button>
                }
              />
            </OverviewCard>}

            <OverviewCard
              key="configure"
              kind="configure"
              onboarding={!showDashboard}
              disabled={!canConfigure && !showDashboard}
              activationReady={!loadingAccount && agents.status === "ready"}
              suspended={!!setup}
            >
              <OverviewArtwork
                kind="configure"
                step={!showDashboard}
                disabled={!canConfigure && !showDashboard}
                onClick={showDashboard ? () => setManagement(true) : openConfigure}
                label={showDashboard ? "Открыть панель управления агентами" : "Выбрать агента для настройки"}
                describedBy="overview-configure-help"
              />
              <OverviewCardContent
                title={showDashboard ? "Управление агентами" : "Настройте агентов"}
                headingLevel={showDashboard ? 2 : 3}
                description={
                  showDashboard ? <>
                    {detectionError && <span>Не все приложения удалось проверить.</span>}
                    <span className="overview-connected-agents" role="list" aria-label="Приложения">
                      {listedAgents.slice(0, 2).map(choice => (
                        <span className="overview-connected-agent" role="listitem" key={choice.id}>
                          <AgentIcon choice={choice} /><span>{choice.name}</span>
                        </span>
                      ))}
                      {listedAgents.length > 2 && <span className="overview-agents-more" role="listitem">… ещё {listedAgents.length - 2}</span>}
                    </span>
                    <span className="overview-manage-hint">Приложения, обновления и настройки подключения.</span>
                  </> : !connected
                    ? "Доступно после подключения аккаунта."
                    : canConfigure
                        ? "Подключите агента к хабу и начните работу."
                        : detectionError
                          ? "Дождитесь проверки установленных агентов."
                          : "Сначала установите хотя бы одного агента."
                }
                descriptionId="overview-configure-help"
                actions={
                  showDashboard ? <Button data-management-target onClick={() => setManagement(true)}>Панель управления</Button> : <Button
                    outline
                    disabled={!canConfigure}
                    aria-describedby="overview-configure-help"
                    onClick={openConfigure}
                  >
                    Настроить агента
                  </Button>
                }
              />
            </OverviewCard>
          </div>
          {account.error && (
            <div className="mt-4" role="alert">
              <ErrorText>{account.error}</ErrorText>
            </div>
          )}
          {connected && detectionError && !showDashboard && (
            <Text className="mt-4" role="status">
              Не все приложения удалось проверить. Список обновится
              автоматически.
            </Text>
          )}
        </section>
      </div>
      {management && connected && (
        <Dialog open size="3xl" onClose={() => { if (!account.agentBusy) setManagement(false); }}>
          <DialogHeader title="Управление агентами" disabled={account.agentBusy} onClose={() => setManagement(false)} />
          <OverviewAgents account={account} agents={agents} />
        </Dialog>
      )}
      {setup && connected && (
        <SetupDialog
          key={setup}
          account={account}
          agents={agents}
          onClose={() => setSetup(undefined)}
        />
      )}
    </div>
  );
}
