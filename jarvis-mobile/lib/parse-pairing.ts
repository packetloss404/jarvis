/**
 * Parse a pairing string into relay URL + session ID.
 *
 * Accepts:
 *   - "jarvis://pair?relay=wss://host/ws&session=abc123&dhpub=..."
 *   - "wss://host/ws|abc123"  (compact format)
 *   - "wss://host/ws"         (bare URL; session id empty until relay assigns one — not used on mobile yet)
 */
function validateRelayUrl(relayUrl: string): void {
  if (!relayUrl.startsWith('wss://')) {
    throw new Error('Relay URL must use wss:// scheme');
  }
}

export function parsePairingString(input: string): { relayUrl: string; sessionId: string; dhPubkey?: string } {
  const trimmed = input.trim();
  if (trimmed.startsWith('jarvis://')) {
    const url = new URL(trimmed);
    const relay = url.searchParams.get('relay') || '';
    const session = url.searchParams.get('session') || '';
    const dhpub = url.searchParams.get('dhpub') || undefined;
    validateRelayUrl(relay);
    return { relayUrl: relay, sessionId: session, dhPubkey: dhpub };
  }

  if (trimmed.includes('|')) {
    const [relayUrl, sessionId] = trimmed.split('|', 2);
    validateRelayUrl(relayUrl);
    return { relayUrl, sessionId };
  }

  validateRelayUrl(trimmed);
  return { relayUrl: trimmed, sessionId: '' };
}
