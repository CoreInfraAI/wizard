import { useRef, useState } from "react";
import { Badge } from "../components/catalyst/badge";
import { Button } from "../components/catalyst/button";
import { Description, Fieldset, Label } from "../components/catalyst/fieldset";
import { Heading } from "../components/catalyst/heading";
import { Switch, SwitchField, SwitchGroup } from "../components/catalyst/switch";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { type ApplicationSetting, setApplicationSetting, useApplicationSettings } from "./application_settings";

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
    </>
  );
}
