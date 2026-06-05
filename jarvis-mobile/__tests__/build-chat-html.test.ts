import { buildChatHTML } from '../lib/jarvis-chat-html';

describe('buildChatHTML', () => {
  it('produces the relay-Room chat bundle without Supabase config', () => {
    const html = buildChatHTML();
    // Supabase transport fully removed.
    expect(html).not.toContain('__JARVIS_MOBILE_SUPABASE_URL__');
    expect(html).not.toContain('__JARVIS_MOBILE_SUPABASE_ANON_KEY__');
    expect(html).not.toContain('createClient');
    expect(html).not.toContain('supabase-js');
    // Relay Room transport present.
    expect(html).toContain('RoomConnection');
    expect(html).toContain('room_hello');
    expect(html).toContain('room_ready');
    // Relay URL is read from the host-injected global.
    expect(html).toContain('window.__JARVIS_RELAY_URL__');
  });
});
