import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "../components/catalyst/button";
import { Heading } from "../components/catalyst/heading";
import { Text } from "../components/catalyst/text";
import { ErrorText } from "../components/ErrorText";

type LogSource = "wizard" | "proxy";

export function LogWindow() {
  const [source, setSource] = useState<LogSource>("wizard");
  const [result, setResult] = useState<{ source: LogSource; text?: string; error?: string }>();
  const text = result?.source === source ? result.text : undefined;
  const error = result?.source === source ? result.error : undefined;
  const fileName = source === "wizard" ? "Wizard.log" : "Proxy.log";
  const [paused, setPaused] = useState(false);
  const [follow, setFollow] = useState(true);
  const output = useRef<HTMLPreElement>(null);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function refresh() {
      try {
        const next = await invoke<string>("read_logs", { source });
        if (!disposed) setResult({ source, text: next });
      } catch (cause: unknown) {
        // Do not log polling failures back into the file being displayed.
        if (!disposed) {
          setResult((previous) => ({
            source,
            text: previous?.source === source ? previous.text : undefined,
            error: `Не удалось прочитать журнал: ${String(cause)}`,
          }));
        }
      } finally {
        if (!disposed && !paused) timer = setTimeout(() => void refresh(), 1000);
      }
    }

    // Always read the selected file once; pause only stops periodic refreshes.
    void refresh();
    return () => {
      disposed = true;
      clearTimeout(timer);
    };
  }, [source, paused]);

  useEffect(() => {
    if (follow && output.current) output.current.scrollTop = output.current.scrollHeight;
  }, [text, follow]);

  return (
    <main className="flex h-dvh flex-col gap-4 bg-zinc-100 p-6 dark:bg-zinc-950">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-4">
          <Heading>Журнал</Heading>
          <div role="group" aria-label="Источник журнала" className="flex gap-2">
            {(["wizard", "proxy"] as const).map((value) => (
              <Button
                key={value}
                outline
                type="button"
                aria-pressed={source === value}
                className={source === value ? "bg-zinc-200 dark:bg-zinc-800" : undefined}
                onClick={() => setSource(value)}
              >
                {value === "wizard" ? "Wizard" : "Прокси"}
              </Button>
            ))}
          </div>
        </div>
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
      <Text>Последние 256 КиБ текущего файла {fileName}. {paused ? "Автообновление приостановлено." : "Обновляется раз в секунду."}</Text>
      {error && <ErrorText>{error}</ErrorText>}
      <pre
        ref={output}
        tabIndex={0}
        aria-label={`Содержимое ${fileName}`}
        className="min-h-0 flex-1 overflow-auto rounded-lg border border-zinc-200 bg-white p-4 font-mono text-xs leading-5 break-words whitespace-pre-wrap text-zinc-900 dark:border-zinc-800 dark:bg-zinc-900 dark:text-zinc-100"
      >
        {text === undefined ? "Загрузка…" : text || "Журнал пока пуст."}
      </pre>
    </main>
  );
}
