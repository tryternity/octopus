import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen as rawListen, type UnlistenFn } from "@tauri-apps/api/event";

export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  return rawInvoke<T>(cmd, args);
}

export async function listen(
  event: string,
  handler: (payload: unknown) => void,
): Promise<UnlistenFn> {
  return rawListen(event, (e) => handler(e.payload));
}
