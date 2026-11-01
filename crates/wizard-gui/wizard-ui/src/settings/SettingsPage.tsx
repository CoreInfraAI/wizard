import { useRef, useState } from "react";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import { Description, Fieldset, Label } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { Switch, SwitchField, SwitchGroup } from "../components/catalyst/switch";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { type ApplicationSetting, setApplicationSetting, useApplicationSettings } from "./application_settings";
import { setSystemProxy, useSystemProxy } from "./system_proxy";

export function SettingsPage() {
  const { settings, error: loadError } = useApplicationSettings();
  const inFlight = useRef(false);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<{ setting: ApplicationSetting; enabled: boolean; message: string }>();
  const disabled = saving || !settings?.supported || Boolean(loadError);

  async function change(setting: ApplicationSetting, enabled: boolean) {
    if (inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    setFailure(undefined);
    try {
      await setApplicationSetting(setting, enabled);
    } catch (cause: unknown) {
      setFailure({ setting, enabled, message: typeof cause === "string" ? cause : "Не удалось изменить настройку приложения" });
    } finally {
      inFlight.current = false;
      setSaving(false);
    }
  }

  return (
    <>
      <Heading>Настройки</Heading>
      <Fieldset className="mt-8 max-w-2xl sm:mt-10" disabled={disabled}>
        {!settings && !loadError && <Badge color="sky" className="mt-4 animate-pulse">Загрузка…</Badge>}
        {settings && !settings.supported && (
          <Text className="mt-4">Работа в трее и автозапуск пока доступны только на macOS и Windows.</Text>
        )}
        <SwitchGroup className="mt-6">
          <SwitchField>
            <Label>Работать в трее</Label>
            <Description>При закрытии окна Wizard продолжает работать в фоне.</Description>
            <Switch checked={settings?.tray_enabled ?? false} onChange={(enabled) => { void change("tray_enabled", enabled); }} />
          </SwitchField>
          <SwitchField>
            <Label>Запускать при входе в систему</Label>
            <Description>При включённой работе в трее Wizard запускается без окна.</Description>
            <Switch checked={settings?.autostart_enabled ?? false} onChange={(enabled) => { void change("autostart_enabled", enabled); }} />
          </SwitchField>
        </SwitchGroup>
      </Fieldset>
      <div className="mt-4 max-w-2xl" aria-live="polite">
        {saving && <Text>Применение…</Text>}
        {loadError && <ErrorText>{loadError}</ErrorText>}
        {failure && (
          <>
            <ErrorText>{failure.message}</ErrorText>
            <Button outline className="mt-3" disabled={disabled}
              onClick={() => { void change(failure.setting, failure.enabled); }}>Повторить</Button>
          </>
        )}
      </div>
      <SystemProxySettings />
    </>
  );
}

function SystemProxySettings() {
  const { state, error: loadError } = useSystemProxy();
  const inFlight = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const [commandError, setCommandError] = useState<string>();
  const hasOurs = state?.status === "on" || state?.status === "partial";
  const requested = state?.requested === true;
  const checked = requested || hasOurs;
  const busy = submitting || state?.busy === true;
  const disabled = busy || Boolean(loadError) || !state || state.status === "unsupported"
    || (!checked && (state.status !== "off" || state.conflict));

  const forceDisabled = busy || Boolean(loadError) || !(hasOurs || state?.status === "off");

  async function change(enabled: boolean, forced = false) {
    if (inFlight.current) return;
    inFlight.current = true;
    setSubmitting(true);
    setCommandError(undefined);
    try {
      await setSystemProxy(enabled, forced);
    } catch (cause: unknown) {
      setCommandError(typeof cause === "string" ? cause : "Не удалось изменить системный прокси");
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }

  return (
    <div className="mt-8 max-w-2xl">
      <SwitchField disabled={disabled}>
        <Label>Системный прокси</Label>
        <Description>Wizard будет проксировать сайты OpenAI и Claude, остальной интернет будет работать как раньше.</Description>
        <Switch checked={checked} onChange={(enabled) => { void change(enabled); }} />
      </SwitchField>
      <div className="mt-3 space-y-2" aria-live="polite">
        {busy && <Text>Применение системных настроек… На macOS может потребоваться авторизация.</Text>}
        {(!state || (state.status === "unknown" && !state.read_error)) && !loadError && <Text>Определение состояния…</Text>}
        {state?.status === "unsupported" && <Text>Системный прокси пока доступен только на macOS и Windows.</Text>}
        {state?.status === "unavailable" && <Text>Нет поддерживаемых сетевых сервисов.</Text>}
        {state?.status === "partial" && (
          <Text>Частично включён: настройки Wizard действуют не для всех протоколов или сетевых сервисов либо конфликтуют с другими настройками. Выключение уберёт только настройки Wizard.</Text>
        )}
        {state?.conflict && <Text>Обнаружены другие настройки прокси, PAC или автоопределение. Обычное включение не изменяет конфликтующие настройки.</Text>}
        {(state?.conflict || state?.status === "partial") && (
          <Button outline disabled={forceDisabled} onClick={() => { void change(true, true); }}>Применить принудительно</Button>
        )}
        {requested && state?.status === "off" && <Text>Спрос на локальный прокси сохранён, но в ОС он выключен. Выключите переключатель, чтобы снять спрос.</Text>}
        {hasOurs && !state?.listener_running && (
          <Text className="text-amber-700! dark:text-amber-400!">ОС использует Wizard, но локальный прокси недоступен. Приложения могут потерять доступ к сети.</Text>
        )}
        {checked && <Text>Прокси продолжит работать после выхода из GUI Wizard.</Text>}
        {loadError && <ErrorText>{loadError}</ErrorText>}
        {state?.read_error && <ErrorText>{state.read_error}</ErrorText>}
        {(state?.operation_error || commandError) && <ErrorText>{state?.operation_error || commandError}</ErrorText>}
      </div>
    </div>
  );
}
