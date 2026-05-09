export function isConfirmationAlreadyResolvedError(error: unknown): boolean {
  const message = String(error ?? "").toLowerCase();
  return message.includes("confirmation not found")
    || message.includes("already rejected")
    || message.includes("already approved");
}
