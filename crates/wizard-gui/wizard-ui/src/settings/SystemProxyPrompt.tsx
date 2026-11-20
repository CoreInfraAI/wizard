import { useEffect } from "react";
import { Button } from "../components/catalyst/button";
import {
  Dialog,
  DialogActions,
  DialogBody,
} from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { useEnableSystemProxy, type useSystemProxy } from "./system_proxy";

export function SystemProxyPrompt({
  observation: { state, error: loadError },
  onClose,
}: {
  observation: ReturnType<typeof useSystemProxy>;
  onClose: () => void;
}) {
  const { enable, submitting, error } = useEnableSystemProxy();
  const busy = submitting || state?.busy === true;
  const failure = error || loadError || state?.read_error;
  const canEnable =
    state &&
    ["off", "partial", "on"].includes(state.status) &&
    !state.conflict &&
    !state.read_error &&
    !loadError;

  // The first observation can arrive after the subscription was selected.
  useEffect(() => {
    if (state?.status === "on" && !busy && !error && !state.operation_error)
      onClose();
  }, [state, busy, error, onClose]);

  return (
    <Dialog
      open
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <DialogHeader
        title="Включить системный прокси?"
        disabled={busy}
        onClose={onClose}
      />
      <DialogBody className="space-y-4">
        <Text>
          Системный прокси направляет запросы к OpenAI и Anthropic через
          выходные узлы CoreInfra AI Hub для входа и работы со своей подпиской.
          <br />
          Все остальные ресурсы доступны напрямую.
        </Text>
        <Text>
          Прокси работает и после закрытия окна. Отключить его можно в Wizard:
          «Настройки» → «Системный прокси».
        </Text>
        {busy && (
          <Text role="status">
            Включаем системный прокси… На macOS может потребоваться
            подтверждение администратора.
          </Text>
        )}
        {!busy && (!state || state.status === "unknown") && !failure && (
          <Text role="status">Проверяем состояние системного прокси…</Text>
        )}
        {state?.conflict && (
          <Text>
            Уже настроен другой прокси. Проверьте его в настройках Wizard перед
            включением.
          </Text>
        )}
        {state?.status === "partial" && !state.conflict && (
          <Text>
            Прокси включён только для части подключений. Wizard включит его для
            остальных.
          </Text>
        )}
        {state?.status === "unsupported" && (
          <Text>Системный прокси доступен на macOS, Windows и в GNOME на Linux.</Text>
        )}
        {state?.status === "unavailable" && (
          <Text>
            Нет доступных сетевых подключений. Подключитесь к сети и повторите
            попытку.
          </Text>
        )}
        {failure && (
          <div role="alert">
            <ErrorText>{failure}</ErrorText>
          </div>
        )}
      </DialogBody>
      <DialogActions>
        <Button plain disabled={busy} onClick={onClose}>
          Пропустить
        </Button>
        <Button
          color="blue"
          disabled={busy || !canEnable}
          aria-busy={busy}
          onClick={async () => {
            if (await enable()) onClose();
          }}
        >
          {busy ? "Включаем…" : "Включить"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
