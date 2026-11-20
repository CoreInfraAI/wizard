import { useRef, useState } from "react";
import { Button } from "../components/catalyst/button";
import { Description, Fieldset, Label } from "../components/catalyst/fieldset";
import { PageHeader, Panel } from "../components/ui/Layout";
import {
  Switch,
  SwitchField,
  SwitchGroup,
} from "../components/catalyst/switch";
import { Text } from "../components/catalyst/text";
import { StateNotice } from "../components/ui/StateNotice";
import {
  type ApplicationSetting,
  setApplicationSetting,
  useApplicationSettings,
} from "./application_settings";
import { setSystemProxy, useSystemProxy } from "./system_proxy";

export function SettingsPage() {
  const { settings, error: loadError } = useApplicationSettings();
  const inFlight = useRef(false);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<{
    setting: ApplicationSetting;
    enabled: boolean;
    message: string;
  }>();
  const disabled = saving || !settings?.supported || Boolean(loadError);

  async function change(setting: ApplicationSetting, enabled: boolean) {
    if (inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    setFailure(undefined);
    try {
      await setApplicationSetting(setting, enabled);
    } catch (cause: unknown) {
      setFailure({
        setting,
        enabled,
        message:
          typeof cause === "string"
            ? cause
            : "Не удалось изменить настройку приложения",
      });
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  }

  return (
    <div className="ui-stack ui-route-enter">
      <PageHeader title="Настройки" />
      <Panel title="Приложение">
        <Fieldset disabled={disabled}>
          {!settings && !loadError && (
            <div className="mb-5">
              <StateNotice tone="loading">Загрузка…</StateNotice>
            </div>
          )}
          {settings && !settings.supported && (
            <div className="mb-5">
              <StateNotice>
                Работа в трее и автозапуск пока доступны только на macOS и
                Windows.
              </StateNotice>
            </div>
          )}
          <SwitchGroup className="ui-setting-list">
            <SwitchField>
              <Label>Работать в трее</Label>
              <Description>
                При закрытии окна Wizard продолжает работать в фоне.
              </Description>
              <Switch
                checked={settings?.tray_enabled ?? false}
                onChange={(enabled) => {
                  void change("tray_enabled", enabled);
                }}
              />
            </SwitchField>
            <SwitchField>
              <Label>Запускать при входе в систему</Label>
              <Description>
                При включённой работе в трее Wizard запускается без окна.
              </Description>
              <Switch
                checked={settings?.autostart_enabled ?? false}
                onChange={(enabled) => {
                  void change("autostart_enabled", enabled);
                }}
              />
            </SwitchField>
          </SwitchGroup>
        </Fieldset>
        <div className="mt-5 max-w-2xl empty:hidden">
          {saving && <StateNotice tone="loading">Применение…</StateNotice>}
          {loadError && <StateNotice tone="error">{loadError}</StateNotice>}
          {failure && (
            <StateNotice
              tone="error"
              actions={
                <Button
                  outline
                  disabled={disabled}
                  onClick={() => {
                    void change(failure.setting, failure.enabled);
                  }}
                >
                  Повторить
                </Button>
              }
            >
              {failure.message}
            </StateNotice>
          )}
        </div>
      </Panel>
      <SystemProxySettings />
    </div>
  );
}

function SystemProxySettings() {
  const { state, error: loadError } = useSystemProxy();
  const inFlight = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const [commandError, setCommandError] = useState<string>();
  const hasOurs = state?.status === "on" || state?.status === "partial";
  const busy = submitting || state?.busy === true;
  const disabled =
    busy ||
    Boolean(loadError) ||
    !state ||
    state.status === "unsupported" ||
    (!hasOurs && (state.status !== "off" || state.conflict));

  const forceDisabled =
    busy || Boolean(loadError) || !(hasOurs || state?.status === "off");

  async function change(enabled: boolean, forced = false) {
    if (inFlight.current) return;
    inFlight.current = true;
    setSubmitting(true);
    setCommandError(undefined);
    try {
      await setSystemProxy(enabled, forced);
    } catch (cause: unknown) {
      setCommandError(
        typeof cause === "string"
          ? cause
          : "Не удалось изменить системный прокси",
      );
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }

  return (
    <Panel
      title="Системный прокси"
      description="Для входа в аккаунты Anthropic и OpenAI."
    >
      <SwitchField disabled={disabled}>
        <Label>Использовать прокси Wizard</Label>
        <Description>
          Запросы к OpenAI и Anthropic идут через выходные узлы CoreInfra AI Hub.
          <br />
          Все остальные ресурсы доступны напрямую.
        </Description>
        <Switch
          checked={hasOurs}
          onChange={(enabled) => {
            void change(enabled);
          }}
        />
      </SwitchField>
      <div className="mt-5 space-y-3 empty:hidden" aria-live="polite">
        {busy && (
          <Text>
            Применение системных настроек… На macOS может потребоваться
            авторизация.
          </Text>
        )}
        {(!state || (state.status === "unknown" && !state.read_error)) &&
          !loadError && <Text>Определение состояния…</Text>}
        {state?.status === "unsupported" && (
          <Text>Системный прокси доступен на macOS, Windows и в GNOME на Linux.</Text>
        )}
        {state?.status === "unavailable" && (
          <Text>Нет поддерживаемых сетевых сервисов.</Text>
        )}
        {state?.status === "partial" && (
          <Text>
            Частично включён: настройки Wizard действуют не для всех протоколов
            или сетевых сервисов либо конфликтуют с другими настройками.
            Выключение уберёт только настройки Wizard.
          </Text>
        )}
        {state?.conflict && (
          <Text>
            Обнаружены другие настройки прокси, PAC или автоопределение. Обычное
            включение не изменяет конфликтующие настройки.
          </Text>
        )}
        {(state?.conflict || state?.status === "partial") && (
          <Button
            outline
            disabled={forceDisabled}
            onClick={() => {
              void change(true, true);
            }}
          >
            Применить принудительно
          </Button>
        )}
        {hasOurs && !state?.listener_running && (
          <StateNotice tone="warning">
            ОС использует Wizard, но локальный прокси недоступен. Приложения
            могут потерять доступ к сети.
          </StateNotice>
        )}
        {loadError && <StateNotice tone="error">{loadError}</StateNotice>}
        {state?.read_error && (
          <StateNotice tone="error">{state.read_error}</StateNotice>
        )}
        {(state?.operation_error || commandError) && (
          <StateNotice tone="error">
            {state?.operation_error || commandError}
          </StateNotice>
        )}
      </div>
    </Panel>
  );
}
