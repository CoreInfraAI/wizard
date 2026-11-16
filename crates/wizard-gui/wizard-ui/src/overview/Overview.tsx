import { useEffect, useState } from "react";
import { ArrowUpRightIcon } from "@heroicons/react/20/solid";
import type { useAgentState } from "../agents/agents_state";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Text, TextLink } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import type { HubAccountControl } from "../settings/HubAccount";
import { hubDisplayName } from "../settings/hub_account";
import { choices, isFound } from "../onboarding/agents";
import { SetupDialog } from "../onboarding/SetupDialog";
import { OverviewArtwork } from "./OverviewArtwork";
import { OverviewCard } from "./OverviewCard";
import { OverviewVideo } from "./OverviewVideo";
import { OverviewBackdrop } from "./OverviewBackdrop";
import "./Overview.css";

export function Overview({ account, agents, onboarding, onFinishOnboarding }: {
  account: HubAccountControl;
  agents: ReturnType<typeof useAgentState>;
  onboarding: boolean;
  onFinishOnboarding: () => void;
}) {
  const [setup, setSetup] = useState<"install" | "select">();
  const [registrationOpened, setRegistrationOpened] = useState(false);
  const connected = account.signedIn;
  const pending = account.account?.pending;
  const loadingAccount = !account.account;
  const installed = agents.status === "ready" ? choices.filter((choice) => isFound(choice, agents.data)) : [];
  const canConfigure = connected && installed.length > 0;
  const activeHub = account.account?.profile?.hubs.find((hub) => hub.hub_id === account.account?.active_hub_id);
  const hubName = activeHub?.is_personal ? "Личный хаб" : activeHub ? hubDisplayName(activeHub) : "CoreInfra AI Hub";
  const profile = account.account?.profile;
  const accountDisabled = account.submitting || (!connected && (loadingAccount || account.agentBusy));
  const accountAction = connected ? account.open : account.change;
  const openInstall = () => setSetup("install");
  const openConfigure = () => setSetup("select");
  const detectionError = agents.status === "error" || (agents.status === "ready" && Object.values(agents.data).some((agent) => typeof agent === "object" && agent.status === "error"));

  useEffect(() => { if (!connected) setSetup(undefined); }, [connected]);

  return <div className="overview" data-onboarding={onboarding}>
    <div className="overview-content">
    <header className="overview-hero">
      <OverviewBackdrop />
      <div className="overview-hero-copy">
        <Heading className="font-semibold!"><span className="overview-hero-text">Ура! Wizard установлен.</span></Heading>
        <Text className="mt-2"><span className="overview-hero-text">{connected
          ? installed.length ? "Выберите агента и настройте подключение к хабу." : "Установите приложение и подключите его к хабу."
          : "Осталось подключить аккаунт и настроить агентов."}</span></Text>
      </div>
      <OverviewVideo />
    </header>
    <section aria-labelledby="overview-actions-title" className="overview-actions">
      <div className="overview-actions-header">
        <Heading level={2} id="overview-actions-title" className="font-semibold!">Начало работы</Heading>
        {onboarding && <Button plain className="overview-skip" onClick={onFinishOnboarding}>Пропустить быструю настройку</Button>}
      </div>
      <div className="overview-cards">
        <OverviewCard kind="account" disabled={accountDisabled} suspended={!!setup}>
          <OverviewArtwork kind="account" connected={connected} disabled={accountDisabled} onClick={accountAction}
            label={connected ? `Открыть хаб: ${hubName}` : "Подключить аккаунт CoreInfra Hub"} />
          <div className="overview-card-body">
            <div className="overview-card-copy" key={connected ? "connected" : "login"}>
              <Heading level={3} className="font-semibold!">{connected ? hubName : "Подключите свой аккаунт"}</Heading>
              {connected ? <Text className="mt-2 truncate" title={profile?.verified_email ?? profile?.username}>{profile?.verified_email ?? profile?.username ?? "CoreInfra Hub"}</Text>
                : <Text className="mt-2">{pending
                ? "Завершите вход в браузере. Wizard подключится автоматически."
                : registrationOpened
                  ? "Создайте аккаунт в браузере, затем вернитесь и войдите в хаб."
                  : "Войдите в CoreInfra Hub, чтобы подключить своих агентов."}</Text>}
            </div>
            <div className="overview-card-actions">
              {connected ? <Button outline disabled={accountDisabled} onClick={accountAction}>Открыть хаб<ArrowUpRightIcon /></Button>
                : <Button disabled={accountDisabled} onClick={accountAction}>{loadingAccount ? "Проверяем аккаунт…" : account.submitting ? "Открываем браузер…" : pending ? "Продолжить вход" : "Войти"}</Button>}
              {!connected && !pending && <TextLink href="https://hub.coreinfra.ai/register?source=wizard" aria-disabled={loadingAccount || account.submitting} onClick={(event) => {
                event.preventDefault();
                if (!loadingAccount && !account.submitting) { setRegistrationOpened(true); account.register(); }
              }}>Создать аккаунт</TextLink>}
            </div>
          </div>
        </OverviewCard>

        <OverviewCard kind="install" disabled={!connected} activationReady={!loadingAccount} suspended={!!setup}>
          <OverviewArtwork kind="install" disabled={!connected} onClick={openInstall}
            label="Перейти к установке агентов" describedBy="overview-install-help" />
          <div className="overview-card-body">
            <div className="overview-card-copy">
              <Heading level={3} className="font-semibold!">Установите агентов</Heading>
              <Text className="mt-2" id="overview-install-help">Wizard скачает и установит Claude или ChatGPT приложения!</Text>
            </div>
            <div className="overview-card-actions"><Button outline disabled={!connected} aria-describedby="overview-install-help" onClick={openInstall}>Установить агента</Button></div>
          </div>
        </OverviewCard>

        <OverviewCard kind="configure" disabled={!canConfigure} activationReady={!loadingAccount && agents.status === "ready"} suspended={!!setup}>
          <OverviewArtwork kind="configure" disabled={!canConfigure} onClick={openConfigure}
            label="Выбрать агента для настройки" describedBy="overview-configure-help" />
          <div className="overview-card-body">
            <div className="overview-card-copy">
              <Heading level={3} className="font-semibold!">Настройте агентов</Heading>
              <Text className="mt-2" id="overview-configure-help">{!connected ? "Доступно после подключения аккаунта." : agents.status === "loading" ? "Ищем установленных агентов…" : canConfigure ? "Подключите агента к хабу и начните работу." : detectionError ? "Дождитесь проверки установленных агентов." : "Сначала установите хотя бы одного агента."}</Text>
            </div>
            <div className="overview-card-actions"><Button outline disabled={!canConfigure} aria-describedby="overview-configure-help" onClick={openConfigure}>Настроить агента</Button></div>
          </div>
        </OverviewCard>
      </div>
      {account.error && <div className="mt-4" role="alert"><ErrorText>{account.error}</ErrorText></div>}
      {connected && detectionError && <Text className="mt-4" role="status">Не все приложения удалось проверить. Список обновится автоматически.</Text>}
    </section>
    </div>
    {setup && connected && <SetupDialog key={setup} initialStage={setup} account={account} agents={agents} onClose={() => setSetup(undefined)} onFinished={onFinishOnboarding} />}
  </div>;
}
