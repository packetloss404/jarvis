/**
 * Build-time public env (Expo). Only EXPO_PUBLIC_* values are embedded in the client.
 */
export function getDefaultRelayHint(): string | undefined {
  const v = process.env.EXPO_PUBLIC_DEFAULT_RELAY_URL;
  return typeof v === 'string' && v.trim() ? v.trim() : undefined;
}

/** Production relay WebSocket endpoint (chat Room transport). */
const DEFAULT_CHAT_RELAY_URL = 'wss://jarvis-relay-production-3eb6.up.railway.app/ws';

/**
 * Relay URL the chat WebView connects to. Defaults to the production relay,
 * overridable via EXPO_PUBLIC_DEFAULT_RELAY_URL (the same env the terminal uses).
 */
export function getChatRelayUrl(): string {
  return getDefaultRelayHint() || DEFAULT_CHAT_RELAY_URL;
}

export function isRelayDebugEnabled(): boolean {
  return process.env.EXPO_PUBLIC_RELAY_DEBUG === '1' || process.env.EXPO_PUBLIC_RELAY_DEBUG === 'true';
}
