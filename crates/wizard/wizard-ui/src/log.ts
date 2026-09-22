import {
  debug as pluginDebug,
  error as pluginError,
  info as pluginInfo,
  warn as pluginWarn,
} from "@tauri-apps/plugin-log";

type Logger = (message: string) => Promise<void>;

function write(logger: Logger, message: string) {
  void logger(message).catch(() => undefined);
}

export function debug(message: string) {
  write(pluginDebug, message);
}

export function info(message: string) {
  write(pluginInfo, message);
}

export function warn(message: string) {
  write(pluginWarn, message);
}

export function error(message: string) {
  write(pluginError, message);
}

export function reportError(message: string, cause: unknown) {
  const details = cause instanceof Error ? cause.stack ?? cause.message : String(cause);
  error(`${message}: ${details}`);
}
