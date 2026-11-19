import { useState } from "react";
import { CheckIcon, ClipboardDocumentIcon, CommandLineIcon } from "@heroicons/react/20/solid";
import { Button } from "../components/catalyst/button";
import { Panel } from "../components/ui/Layout";
import { EmptyState } from "../components/ui/StateNotice";
import { ErrorText } from "../components/ErrorText";

const instructions = {
  claude: { name: "Claude Code", command: "npm install -g @anthropic-ai/claude-code" },
  codex: { name: "Codex CLI", command: "npm install -g @openai/codex" },
  opencode: { name: "OpenCode", command: "npm install -g opencode-ai" },
  pi: { name: "Pi", command: "npm install -g @mariozechner/pi-coding-agent" },
};

/** Give terminal agents an actionable empty state without running shell commands. */
export function MissingAgent({ agent }: { agent: keyof typeof instructions }) {
  const { name, command } = instructions[agent];
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string>();
  async function copy() {
    setError(undefined);
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
    } catch {
      setError("Не удалось скопировать. Выделите команду и скопируйте её вручную.");
    }
  }
  return (
    <Panel className="mt-6">
      <EmptyState title={`${name} не установлен`} headingLevel={2} icon={CommandLineIcon}>
        Выполните команду в терминале с установленными Node.js и npm.
        Wizard обнаружит агента автоматически.
      </EmptyState>
      <div className="agent-install-command">
        <code>{command}</code>
        <Button outline size="sm" onClick={() => void copy()}>
          {copied ? <CheckIcon /> : <ClipboardDocumentIcon />}
          {copied ? "Скопировано" : "Скопировать команду"}
        </Button>
      </div>
      {error && <div className="mt-3" role="alert"><ErrorText>{error}</ErrorText></div>}
    </Panel>
  );
}
