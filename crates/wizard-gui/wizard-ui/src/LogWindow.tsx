import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "./components/catalyst/button";
import { Heading } from "./components/catalyst/heading";
import { Text } from "./components/catalyst/text";
import { ErrorText } from "./components/ErrorText";

export function LogWindow() {
  const [text, setText] = useState<string>();
  const [error, setError] = useState<string>();
  const [paused, setPaused] = useState(false);
  const [follow, setFollow] = useState(true);
  const output = useRef<HTMLPreElement>(null);

  useEffect(() => {
    if (paused) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function refresh() {
      try {
        const next = await invoke<string>("read_logs");
        if (!disposed) {
          setText(next);
          setError(undefined);
        }
      } catch (cause: unknown) {
        // Do not log polling failures back into the file being displayed.
        if (!disposed) setError(`Не удалось прочитать журнал: ${String(cause)}`);
      } finally {
        if (!disposed) timer = setTimeout(() => void refresh(), 1000);
      }
    }

    void refresh();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [paused]);

  useEffect(() => {
    if (follow && output.current) output.current.scrollTop = output.current.scrollHeight;
  }, [text, follow]);

  return (
    <main className="flex h-dvh flex-col gap-4 p-6">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <Heading>Журнал</Heading>
        <div className="flex flex-wrap items-center gap-4">
          <label className="flex items-center gap-2 text-sm text-zinc-700 dark:text-zinc-300">
            <input type="checkbox" checked={follow} onChange={(event) => setFollow(event.target.checked)} />
            Прокручивать вниз
          </label>
          <Button outline type="button" onClick={() => setPaused((value) => !value)}>
            {paused ? "Продолжить" : "Пауза"}
          </Button>
        </div>
      </div>
      <Text>Последние 256 КиБ текущего файла Wizard.log. {paused ? "Обновление приостановлено." : "Обновляется раз в секунду."}</Text>
      {error && <ErrorText>{error}</ErrorText>}
      <pre
        ref={output}
        tabIndex={0}
        aria-label="Содержимое журнала"
        className="min-h-0 flex-1 overflow-auto rounded-lg border border-zinc-200 bg-white p-4 font-mono text-xs leading-5 break-words whitespace-pre-wrap text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-100"
      >
        {text === undefined ? "Загрузка…" : text || "Журнал пока пуст."}
      </pre>
    </main>
  );
}
