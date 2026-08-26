import { invoke } from "@tauri-apps/api/core";

interface OperationOptions {
  signal?: AbortSignal;
}

function cancelledError(): DOMException {
  return new DOMException("The operation was cancelled.", "AbortError");
}

export async function invokeOperation<T>(
  command: string,
  args: Record<string, unknown>,
  options: OperationOptions = {},
): Promise<T> {
  const { signal } = options;
  if (signal?.aborted) throw cancelledError();
  const operationId = `ui-${crypto.randomUUID()}`;
  const cancel = () => {
    void invoke("cancel_operation", { operationId }).catch(() => {
      // The primary invocation reports transport failures. Cancellation is
      // deliberately best-effort when the operation has already completed.
    });
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    const result = await invoke<T>(command, { ...args, operationId });
    if (signal?.aborted) throw cancelledError();
    return result;
  } finally {
    signal?.removeEventListener("abort", cancel);
  }
}
