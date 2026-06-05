import { buildChatHTML } from '../lib/jarvis-chat-html';

/**
 * Golden-vector conformance for the signed `room_hello` canonical payload.
 *
 * The relay (Rust: `jarvis-relay/src/room_auth.rs`) and ALL Room clients must
 * produce byte-identical canonical signing bytes. The shared fixed vector is
 *
 *   signed_hello_payload("sid", "m1", "pk", 42)
 *     == base64("jarvis-room-hello-v1" 0x1F "sid" 0x1F "m1" 0x1F "pk" 0x1F "42")
 *     == "amFydmlzLXJvb20taGVsbG8tdjEfc2lkH20xH3BrHzQy"
 *
 * The Rust side asserts the same constant (`golden_signed_hello_payload_vector`
 * in room_auth.rs). This test guards the mobile bundle so a future SEP / domain
 * drift (e.g. the `'\\x1F'` four-char bug that broke mobile chat) fails the build.
 */
const GOLDEN_PAYLOAD = 'amFydmlzLXJvb20taGVsbG8tdjEfc2lkH20xH3BrHzQy';

// Reproduce the EXACT logic the bundle's RoomHelloSig.payload uses, so this test
// independently computes what the client signs (no reliance on the bundle text).
const DOMAIN = 'jarvis-room-hello-v1';
const SEP = '\x1F'; // single ASCII Unit Separator byte (0x1F)

function btoaUtf8(s: string): string {
  // The bundle runs in a WebView where btoa is global; under Node/jest we use
  // Buffer with 'binary' (latin1) — matching btoa, since every byte here is < 0x80.
  return Buffer.from(s, 'binary').toString('base64');
}

function canonicalPayload(
  sessionId: string,
  memberId: string,
  pubkey: string,
  nonce: number,
): string {
  const canonical =
    DOMAIN + SEP + sessionId + SEP + memberId + SEP + pubkey + SEP + String(nonce);
  return btoaUtf8(canonical);
}

describe('signed room_hello canonical payload (mobile)', () => {
  it('matches the shared golden vector', () => {
    expect(canonicalPayload('sid', 'm1', 'pk', 42)).toBe(GOLDEN_PAYLOAD);
  });

  it('emits SEP as a single 0x1F byte (not the 4-char "\\x1F" literal)', () => {
    // The bundle is a backtick template, so the source `'\x1F'` is resolved to
    // the actual 0x1F byte in the built HTML. The buggy `'\\x1F'` would instead
    // leave the literal 4-char source text `\x1F` in the bundle.
    const html = buildChatHTML();
    // Single 0x1F byte present in the SEP assignment.
    expect(html).toContain(`SEP: '${SEP}'`);
    // The buggy double-escaped source text must NOT appear.
    expect(html).not.toContain("SEP: '\\x1F'");
  });

  it('SEP in the bundle is exactly one 0x1F byte', () => {
    const html = buildChatHTML();
    const m = html.match(/SEP:\s*'([^']*)'/);
    expect(m).not.toBeNull();
    const sepRuntime = m![1];
    expect(sepRuntime.length).toBe(1);
    expect(sepRuntime.charCodeAt(0)).toBe(0x1f);
    expect(sepRuntime).toBe(SEP);
  });
});
