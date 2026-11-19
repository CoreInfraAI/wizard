import { useId, type CSSProperties, type ReactNode } from "react";
import { CheckIcon, ExclamationCircleIcon } from "@heroicons/react/20/solid";
import "./TransferCard.css";

type TransferCardProps = {
  name: string;
  icon: ReactNode;
  brand: "claude" | "chatgpt" | "wizard";
  version?: string;
  status: ReactNode;
  detail?: ReactNode;
  notice?: ReactNode;
  error?: ReactNode;
  progress?: number;
  downloading?: boolean;
  filled?: boolean;
  tone?: "default" | "success" | "error";
  variant?: "card" | "compact";
  action?: ReactNode;
  onOpen?: () => void;
};

/** One presentation for a selected installer, a picker card, and a sidebar job. */
export function TransferCard({
  name,
  icon,
  brand,
  version,
  status,
  detail,
  notice,
  error,
  progress,
  downloading = false,
  filled = false,
  tone = "default",
  variant = "card",
  action,
  onOpen,
}: TransferCardProps) {
  const descriptionId = useId();
  const fill = filled ? 100 : Math.min(100, Math.max(0, progress ?? 0));
  const compact = variant === "compact";
  const Container = compact ? "span" : "div";
  const Name = compact ? "span" : "h3";
  const contents = (
    <>
      <span
        className="ui-transfer-fill"
        aria-hidden="true"
        style={{ width: `${fill}%` } as CSSProperties}
      />
      <Container className="ui-transfer-body">
        <Container className="ui-transfer-content">
          <Container className="ui-transfer-header">
            <span className="ui-transfer-logo" aria-hidden="true">
              {icon}
            </span>
            <Container className="ui-transfer-copy">
              <Container className="ui-transfer-title">
                <Name>{name}</Name>
                {compact &&
                  (downloading ? (
                    <span className="ui-transfer-percent">
                      {progress === undefined
                        ? "…"
                        : `${Math.floor(progress)}%`}
                    </span>
                  ) : tone === "success" ? (
                    <CheckIcon aria-hidden="true" />
                  ) : tone === "error" ? (
                    <ExclamationCircleIcon aria-hidden="true" />
                  ) : null)}
              </Container>
              {!compact && version && (
                <Container className="ui-transfer-version">{version}</Container>
              )}
              <Container
                id={descriptionId}
                className="ui-transfer-status"
                // The progressbar exposes changing values on demand. Announce
                // phase changes without reading every download tick aloud.
                role={compact || downloading ? undefined : "status"}
              >
                {status}
              </Container>
              {detail && (
                <Container className="ui-transfer-detail">{detail}</Container>
              )}
              {notice && (
                <Container className="ui-transfer-notice">{notice}</Container>
              )}
            </Container>
          </Container>
          {error && (
            <Container className="ui-transfer-error" role="alert">
              {error}
            </Container>
          )}
        </Container>
        {!compact && action && (
          <div className="ui-transfer-actions">{action}</div>
        )}
      </Container>
      {downloading && (
        <progress
          className="sr-only"
          aria-label={`Скачивание ${name}`}
          max={100}
          value={progress}
        />
      )}
    </>
  );
  const attributes = {
    className: "ui-transfer",
    "data-variant": variant,
    "data-brand": brand,
    "data-tone": tone,
    "data-active": downloading || filled,
    "data-openable": Boolean(onOpen),
  };
  return compact ? (
    <button
      {...attributes}
      type="button"
      aria-label={`Открыть установку ${name}`}
      aria-describedby={descriptionId}
      onClick={onOpen}
    >
      {contents}
    </button>
  ) : (
    <article {...attributes} aria-label={name} onClick={onOpen}>
      {contents}
    </article>
  );
}
