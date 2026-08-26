/** Render any thrown value (Error, IPC string, or unknown) as a user-facing message. */
export function toMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
