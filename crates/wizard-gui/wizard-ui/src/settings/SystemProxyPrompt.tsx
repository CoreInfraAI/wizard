import { useEffect, useRef, useState } from "react";
import { Button } from "../components/catalyst/button";
import {
  Dialog,
  DialogActions,
  DialogBody,
} from "../components/catalyst/dialog";
import { DialogHeader } from "../components/ui/DialogHeader";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";
import { setSystemProxy, type useSystemProxy } from "./system_proxy";

export function SystemProxyPrompt({
  observation: { state, error: loadError },
  onClose,
}: {
  observation: ReturnType<typeof useSystemProxy>;
  onClose: () => void;
}) {
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string>();
  const inFlight = useRef(false);
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

  async function enable() {
    if (inFlight.current || busy || !canEnable) return;
    inFlight.current = true;
    setSubmitting(true);
    setError(undefined);
    try {
      // Use the same verified OS operation as Settings; never overwrite another proxy.
      await setSystemProxy(true);
      onClose();
    } catch (cause) {
      setError(
        typeof cause === "string"
          ? cause
          : "Не удалось включить системный прокси. Попробуйте ещё раз.",
      );
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }

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
          Отмена
        </Button>
        <Button
          disabled={busy || !canEnable}
          aria-busy={busy}
          onClick={() => void enable()}
        >
          {busy ? "Включаем…" : "Включить"}
        </Button>
      </DialogActions>
    </Dialog>
  );
}
