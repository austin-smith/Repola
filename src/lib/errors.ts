/** Render any thrown value (Error, IPC string or object, or unknown) as a user-facing message. */
export function toMessage(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "object" && cause !== null && "message" in cause && typeof cause.message === "string") return cause.message;
  return String(cause);
}
