import { useState } from "react";
import { sendAgentEvent } from "./agents_state";
import { reportError } from "./log";

export function CoreinfraToken({ saved }: { saved: boolean }) {
  const [token, setToken] = useState("");
  const [error, setError] = useState<string>();

  async function save() {
    const submitted = token;
    setError(undefined);
    try {
      await sendAgentEvent({ SetCoreinfraToken: submitted.trim() });
      setToken((current) => current === submitted ? "" : current);
    } catch (cause: unknown) {
      reportError("failed to save CoreInfra token", cause);
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <section>
      <h2>CoreInfra API token</h2>
      <div>
        <label htmlFor="coreinfra-token">Токен</label>{" "}
        <input
          id="coreinfra-token"
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={token}
          onChange={(event) => {
            setToken(event.target.value);
            setError(undefined);
          }}
        />{" "}
        <button type="button" onClick={() => void save()}>
          Сохранить
        </button>
      </div>
      <p>{saved ? "Токен сохранён." : "Токен не задан."}</p>
      {error && <p>Не удалось сохранить токен: {error}</p>}
    </section>
  );
}
